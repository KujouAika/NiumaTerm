// NiumaTerm remote-session relay.
//
// A host keeps a control socket open here. When a client connects, the
// host's room tells the host over that socket, the host opens a data socket
// for that client, and the room forwards binary frames between the two
// without reading them. Every frame is sealed end to end by the Noise
// channel the host and client run, so the relay sees only ciphertext and a
// compromised relay can cost availability, never confidentiality.
//
// Endpoints (WebSocket upgrades, all require `Authorization: Bearer
// <ACCESS_KEY>`):
//   /v1/host/{host_id}               host control socket (+ X-Host-Token)
//   /v1/host/{host_id}/accept/{conn} host data socket for one client
//   /v1/client/{host_id}             client connection
//   /v1/pair/{slot}                  client connection to the host showing
//                                    a pairing code with this slot

import { DurableObject } from "cloudflare:workers";

export interface Env {
  ACCESS_KEY: string;
  HOST_ROOM: DurableObjectNamespace<HostRoom>;
  DIRECTORY: DurableObjectNamespace<PairingDirectory>;
}

/// Client connections one host may hold at once.
const MAX_CLIENTS = 8;

/// Largest frame forwarded. Channel frames are at most 64 KiB of ciphertext.
const MAX_MESSAGE = 128 * 1024;

/// A client that sees no `open` within this long gets close code 4404.
const OPEN_TIMEOUT_MS = 10_000;

/// Connection attempts allowed per client IP per minute. Counted per
/// isolate, so this bounds abuse without a shared store.
const ATTEMPTS_PER_MINUTE = 30;

const HOST_ID = /^[0-9a-z]{16}$/;
const SLOT = /^[0-9A-Z]{3}$/;
const CONN = /^[0-9a-f]{16}$/;

const attempts = new Map<string, { windowStart: number; count: number }>();

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    if (!authorized(request, env)) {
      return new Response("unauthorized", { status: 401 });
    }

    if (request.headers.get("Upgrade") !== "websocket") {
      return new Response("expected a WebSocket upgrade", { status: 426 });
    }

    if (!admit(request.headers.get("CF-Connecting-IP") ?? "unknown")) {
      return new Response("too many attempts", { status: 429 });
    }

    const parts = new URL(request.url).pathname.split("/").filter(Boolean);

    if (parts[0] !== "v1") {
      return new Response("not found", { status: 404 });
    }

    // /v1/pair/{slot}: resolve the slot to its host, then join as a client.
    if (parts[1] === "pair" && parts.length === 3 && SLOT.test(parts[2])) {
      const directory = env.DIRECTORY.get(env.DIRECTORY.idFromName("directory"));
      const hostId = await directory.resolve(parts[2]);

      if (hostId === null) {
        return new Response("no host is showing that code", { status: 404 });
      }

      return room(env, hostId).fetch(roomRequest(request, "client", hostId));
    }

    const hostId = parts[2];

    if (!HOST_ID.test(hostId ?? "")) {
      return new Response("not found", { status: 404 });
    }

    if (parts[1] === "client" && parts.length === 3) {
      return room(env, hostId).fetch(roomRequest(request, "client", hostId));
    }

    if (parts[1] === "host" && parts.length === 3) {
      return room(env, hostId).fetch(roomRequest(request, "control", hostId));
    }

    if (parts[1] === "host" && parts.length === 5 && parts[3] === "accept" && CONN.test(parts[4])) {
      return room(env, hostId).fetch(roomRequest(request, `accept:${parts[4]}`, hostId));
    }

    return new Response("not found", { status: 404 });
  },
};

function authorized(request: Request, env: Env): boolean {
  const presented = request.headers.get("Authorization") ?? "";
  const expected = `Bearer ${env.ACCESS_KEY ?? ""}`;

  // Without a configured key the relay admits nobody.
  return (env.ACCESS_KEY ?? "").length > 0 && constantTimeEqual(presented, expected);
}

function constantTimeEqual(a: string, b: string): boolean {
  if (a.length !== b.length) {
    return false;
  }

  let difference = 0;

  for (let i = 0; i < a.length; i++) {
    difference |= a.charCodeAt(i) ^ b.charCodeAt(i);
  }

  return difference === 0;
}

function admit(ip: string): boolean {
  const now = Date.now();
  const entry = attempts.get(ip);

  if (entry === undefined || now - entry.windowStart >= 60_000) {
    attempts.set(ip, { windowStart: now, count: 1 });

    return true;
  }

  entry.count += 1;

  return entry.count <= ATTEMPTS_PER_MINUTE;
}

function room(env: Env, hostId: string): DurableObjectStub<HostRoom> {
  return env.HOST_ROOM.get(env.HOST_ROOM.idFromName(hostId));
}

function roomRequest(request: Request, kind: string, hostId: string): Request {
  const forwarded = new Request(request);

  forwarded.headers.set("X-Relay-Kind", kind);
  forwarded.headers.set("X-Relay-Host", hostId);

  return forwarded;
}

async function sha256Hex(text: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));

  return [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

function randomConn(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(8));

  return [...bytes].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

interface ClientAttachment {
  conn: string;
  opened: boolean;
  since: number;
}

/// The sockets of one host: its control socket, its clients, and the data
/// socket it opens for each client. Uses the WebSocket Hibernation API, so an
/// idle room costs nothing while its sockets stay open.
export class HostRoom extends DurableObject<Env> {
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);

    // Keep-alive pings are answered without waking the object.
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
  }

  async fetch(request: Request): Promise<Response> {
    const kind = request.headers.get("X-Relay-Kind") ?? "";
    const hostId = request.headers.get("X-Relay-Host") ?? "";

    if (kind === "client") {
      return this.acceptClient();
    }

    // Host sockets prove they belong to the host that registered first.
    if (!(await this.verifyHost(request.headers.get("X-Host-Token") ?? ""))) {
      return new Response("this host id belongs to another host", { status: 403 });
    }

    if (kind === "control") {
      return this.acceptControl(hostId);
    }

    if (kind.startsWith("accept:")) {
      return this.acceptData(kind.slice("accept:".length));
    }

    return new Response("not found", { status: 404 });
  }

  /// Trust on first use: the first control connection for a host id stores
  /// a hash of its token, and later host sockets must present the same one.
  /// A host id is only disclosed through pairing, which happens after the
  /// host has registered, so squatting would need the id beforehand.
  private async verifyHost(token: string): Promise<boolean> {
    if (token.length < 32) {
      return false;
    }

    const hash = await sha256Hex(token);
    const stored = await this.ctx.storage.get<string>("tokenHash");

    if (stored === undefined) {
      await this.ctx.storage.put("tokenHash", hash);

      return true;
    }

    return constantTimeEqual(stored, hash);
  }

  private acceptControl(hostId: string): Response {
    // A reconnecting host replaces its previous control socket.
    for (const old of this.ctx.getWebSockets("control")) {
      old.close(1000, "replaced");
    }

    const [client, server] = Object.values(new WebSocketPair());

    this.ctx.acceptWebSocket(server, ["control"]);
    server.serializeAttachment({ hostId });

    return new Response(null, { status: 101, webSocket: client });
  }

  private acceptClient(): Response {
    const [client, server] = Object.values(new WebSocketPair());
    const control = this.ctx.getWebSockets("control")[0];

    if (control === undefined) {
      this.ctx.acceptWebSocket(server, ["refused"]);
      server.close(4404, "host offline");

      return new Response(null, { status: 101, webSocket: client });
    }

    if (this.ctx.getWebSockets("client").length >= MAX_CLIENTS) {
      this.ctx.acceptWebSocket(server, ["refused"]);
      server.close(4429, "too many clients");

      return new Response(null, { status: 101, webSocket: client });
    }

    const conn = randomConn();

    this.ctx.acceptWebSocket(server, ["client", `c:${conn}`]);
    server.serializeAttachment({ conn, opened: false, since: Date.now() } satisfies ClientAttachment);

    control.send(JSON.stringify({ t: "conn", id: conn }));

    // The client starts its handshake only after `open`, so nothing needs
    // buffering here; one that never gets `open` is closed.
    this.ctx.storage.setAlarm(Date.now() + OPEN_TIMEOUT_MS);

    return new Response(null, { status: 101, webSocket: client });
  }

  private acceptData(conn: string): Response {
    const client = this.ctx.getWebSockets(`c:${conn}`)[0];

    if (client === undefined) {
      return new Response("that client is gone", { status: 404 });
    }

    const [pair0, server] = Object.values(new WebSocketPair());

    this.ctx.acceptWebSocket(server, [`h:${conn}`]);

    const attachment = client.deserializeAttachment() as ClientAttachment;

    client.serializeAttachment({ ...attachment, opened: true });
    client.send(JSON.stringify({ t: "open" }));

    return new Response(null, { status: 101, webSocket: pair0 });
  }

  async webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): Promise<void> {
    const tags = this.ctx.getTags(ws);

    if (tags.includes("control")) {
      if (typeof message === "string") {
        await this.onControl(ws, message);
      }

      return;
    }

    if (typeof message !== "string" && message.byteLength > MAX_MESSAGE) {
      ws.close(1009, "message too large");

      return;
    }

    const peer = this.peerOf(tags);

    if (peer !== undefined) {
      peer.send(message);
    }
  }

  async webSocketClose(ws: WebSocket, code: number, reason: string): Promise<void> {
    this.closePeer(ws, code, reason);
  }

  async webSocketError(ws: WebSocket): Promise<void> {
    this.closePeer(ws, 1011, "peer error");
  }

  async alarm(): Promise<void> {
    const now = Date.now();
    let waiting = false;

    for (const client of this.ctx.getWebSockets("client")) {
      const attachment = client.deserializeAttachment() as ClientAttachment;

      if (attachment.opened) {
        continue;
      }

      if (now - attachment.since >= OPEN_TIMEOUT_MS) {
        client.close(4404, "host not answering");
      } else {
        waiting = true;
      }
    }

    if (waiting) {
      await this.ctx.storage.setAlarm(now + 1_000);
    }
  }

  /// Pairing slots are claimed and released over the control socket.
  private async onControl(ws: WebSocket, message: string): Promise<void> {
    let request: { t?: string; slot?: string; ttl?: number };

    try {
      request = JSON.parse(message);
    } catch {
      return;
    }

    const slot = request.slot ?? "";

    if (!SLOT.test(slot)) {
      return;
    }

    const { hostId } = ws.deserializeAttachment() as { hostId: string };
    const directory = this.env.DIRECTORY.get(this.env.DIRECTORY.idFromName("directory"));

    if (request.t === "slot") {
      const ttl = Math.min(Math.max(request.ttl ?? 300, 1), 600);
      const claimed = await directory.claim(slot, hostId, ttl);

      ws.send(JSON.stringify({ t: claimed ? "slot_ok" : "slot_taken", slot }));
    } else if (request.t === "slot_release") {
      await directory.release(slot, hostId);
    }
  }

  private peerOf(tags: string[]): WebSocket | undefined {
    for (const tag of tags) {
      if (tag.startsWith("c:")) {
        return this.ctx.getWebSockets(`h:${tag.slice(2)}`)[0];
      }

      if (tag.startsWith("h:")) {
        return this.ctx.getWebSockets(`c:${tag.slice(2)}`)[0];
      }
    }

    return undefined;
  }

  /// Closing either side of a client connection closes the other.
  private closePeer(ws: WebSocket, code: number, reason: string): void {
    const peer = this.peerOf(this.ctx.getTags(ws));
    const forwarded = code === 1005 || code === 1006 ? 1000 : code;

    try {
      peer?.close(forwarded, reason);
    } catch {
      // Already closing.
    }
  }
}

/// Maps pairing slots to the host showing the code, until the code expires.
export class PairingDirectory extends DurableObject<Env> {
  async claim(slot: string, hostId: string, ttlSeconds: number): Promise<boolean> {
    const now = Date.now();
    const existing = await this.ctx.storage.get<{ hostId: string; expires: number }>(slot);

    if (existing !== undefined && existing.expires > now && existing.hostId !== hostId) {
      return false;
    }

    await this.ctx.storage.put(slot, { hostId, expires: now + ttlSeconds * 1000 });

    return true;
  }

  async release(slot: string, hostId: string): Promise<void> {
    const existing = await this.ctx.storage.get<{ hostId: string; expires: number }>(slot);

    if (existing?.hostId === hostId) {
      await this.ctx.storage.delete(slot);
    }
  }

  async resolve(slot: string): Promise<string | null> {
    const existing = await this.ctx.storage.get<{ hostId: string; expires: number }>(slot);

    if (existing === undefined || existing.expires <= Date.now()) {
      return null;
    }

    return existing.hostId;
  }
}
