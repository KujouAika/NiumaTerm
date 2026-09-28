// Push notifications for the NiumaTerm phone app.
//
// A host that has something to tell a paired phone (an agent finished,
// needs approval, asks a question) seals the text for that phone and posts
// it here. The relay signs an APNs request with the app's push key and
// forwards the sealed bytes; the phone's notification extension opens them.
// The relay never sees the text: it only knows a device token, a host id,
// and ciphertext.
//
// The APNs key belongs to the developer account that signs the app, so one
// relay, the one whose deployment holds that key, forwards pushes for every
// host, including hosts that use other relays for their sessions. That is
// why this endpoint takes no access key: hosts it has never seen must reach
// it. What it forwards is bounded instead: the topic is fixed here, so it
// only reaches this app, every payload is ciphertext only the phone's own
// key opens, and each device token has a rate limit.
//
//   POST /v1/push
//   { "token": "<hex>", "environment": "sandbox" | "production",
//     "host": "<host id>", "sealed": "<base64>", "collapse": "<session id>" }
//
// Answers 200 with `{ "status": <APNs status>, "reason"?: "<APNs reason>" }`
// once APNs answered, 400 for a malformed request, 429 when the token's rate
// limit is spent, and 501 when this relay has no push key configured.

import { DurableObject } from "cloudflare:workers";

export interface PushEnv {
  /// The `.p8` key from the developer account, PEM text included.
  APNS_KEY?: string;
  APNS_KEY_ID?: string;
  APNS_TEAM_ID?: string;
  /// The app's bundle identifier.
  APNS_TOPIC?: string;
  PUSH_GATEWAY: DurableObjectNamespace<PushGateway>;
}

export interface Push {
  token: string;
  environment: "sandbox" | "production";
  host: string;
  sealed: string;
  collapse?: string;
}

export interface PushResult {
  /// The token spent its rate limit; nothing was sent.
  limited: boolean;
  status: number;
  reason?: string;
}

const DEVICE_TOKEN = /^[0-9a-f]{64,200}$/;
const HOST_ID = /^[0-9a-z]{16}$/;
const COLLAPSE_ID = /^[0-9A-Za-z_-]{1,64}$/;
const BASE64 = /^[0-9A-Za-z+/]+={0,2}$/;

/// Sealed bodies stay under 3 KB, so with the `aps` dictionary a push fits
/// the 4 KB APNs payload limit.
const MAX_SEALED = 3072;

/// Larger requests are rejected before they are read.
const MAX_REQUEST = 8192;

/// A host pushes on turn ends, approvals and questions; a minute holding
/// more than this for one phone is a runaway loop or abuse.
const PUSHES_PER_TOKEN_PER_MINUTE = 60;

/// APNs rejects provider tokens older than an hour and throttles ones
/// refreshed more often than every 20 minutes.
const PROVIDER_TOKEN_LIFETIME_MS = 50 * 60 * 1000;

export async function handlePush(request: Request, env: PushEnv): Promise<Response> {
  if (request.method !== "POST") {
    return new Response("expected POST", { status: 405 });
  }

  if (!env.APNS_KEY || !env.APNS_KEY_ID || !env.APNS_TEAM_ID || !env.APNS_TOPIC) {
    return new Response("push is not set up on this relay", { status: 501 });
  }

  if (Number(request.headers.get("Content-Length") ?? MAX_REQUEST + 1) > MAX_REQUEST) {
    return new Response("request too large", { status: 413 });
  }

  let push: Push;

  try {
    push = parsePush(await request.json());
  } catch (error) {
    return new Response(`bad push: ${(error as Error).message}`, { status: 400 });
  }

  // One gateway holds the provider token for every push: separate isolates
  // each signing their own would refresh it faster than APNs allows.
  const gateway = env.PUSH_GATEWAY.get(env.PUSH_GATEWAY.idFromName("apns"));
  const result = await gateway.send(push);

  if (result.limited) {
    return new Response("too many pushes for this device", { status: 429 });
  }

  return Response.json({ status: result.status, reason: result.reason });
}

function parsePush(value: unknown): Push {
  const push = value as Partial<Push>;

  if (typeof push.token !== "string" || !DEVICE_TOKEN.test(push.token)) {
    throw new Error("token");
  }

  if (push.environment !== "sandbox" && push.environment !== "production") {
    throw new Error("environment");
  }

  if (typeof push.host !== "string" || !HOST_ID.test(push.host)) {
    throw new Error("host");
  }

  if (
    typeof push.sealed !== "string" ||
    push.sealed.length > MAX_SEALED ||
    !BASE64.test(push.sealed)
  ) {
    throw new Error("sealed");
  }

  if (push.collapse !== undefined && (typeof push.collapse !== "string" || !COLLAPSE_ID.test(push.collapse))) {
    throw new Error("collapse");
  }

  return {
    token: push.token,
    environment: push.environment,
    host: push.host,
    sealed: push.sealed,
    collapse: push.collapse,
  };
}

interface ProviderToken {
  value: string;
  issuedAt: number;
  /// The key that signed it, so replacing the key replaces the token at
  /// once instead of when it would have expired.
  keyId: string;
}

/// Signs and sends every push. A single instance keeps the APNs provider
/// token, in storage so a restarted instance reuses it instead of signing a
/// new one early, and counts pushes per device token.
export class PushGateway extends DurableObject<PushEnv> {
  private providerToken: ProviderToken | null = null;
  private counts = new Map<string, { windowStart: number; count: number }>();

  async send(push: Push): Promise<PushResult> {
    if (!this.admit(push.token)) {
      return { limited: true, status: 429 };
    }

    const token = await this.currentProviderToken();
    const host = push.environment === "production" ? "api.push.apple.com" : "api.sandbox.push.apple.com";

    const headers: Record<string, string> = {
      authorization: `bearer ${token}`,
      "apns-topic": this.env.APNS_TOPIC!,
      "apns-push-type": "alert",
      "apns-priority": "10",
    };

    if (push.collapse !== undefined) {
      headers["apns-collapse-id"] = push.collapse;
    }

    // The visible text is a placeholder. The notification extension opens
    // `s` with the push key it holds for host `h` and replaces it; if it
    // cannot, the placeholder is still a notification, only less specific.
    const payload = {
      aps: {
        alert: { title: "NiumaTerm", body: "Agent update" },
        "mutable-content": 1,
        sound: "default",
        ...(push.collapse !== undefined ? { "thread-id": push.collapse } : {}),
      },
      h: push.host,
      s: push.sealed,
    };

    const response = await fetch(`https://${host}/3/device/${push.token}`, {
      method: "POST",
      headers,
      body: JSON.stringify(payload),
    });

    let reason: string | undefined;

    if (!response.ok) {
      try {
        reason = ((await response.json()) as { reason?: string }).reason;
      } catch {
        reason = undefined;
      }

      // A token APNs no longer accepts is replaced on the next push.
      if (reason === "ExpiredProviderToken" || reason === "InvalidProviderToken") {
        this.providerToken = null;
        await this.ctx.storage.delete("providerToken");
      }
    }

    return { limited: false, status: response.status, reason };
  }

  private admit(deviceToken: string): boolean {
    const now = Date.now();
    const entry = this.counts.get(deviceToken);

    if (entry === undefined || now - entry.windowStart >= 60_000) {
      this.counts.set(deviceToken, { windowStart: now, count: 1 });

      return true;
    }

    entry.count += 1;

    return entry.count <= PUSHES_PER_TOKEN_PER_MINUTE;
  }

  private async currentProviderToken(): Promise<string> {
    const now = Date.now();

    this.providerToken ??= (await this.ctx.storage.get<ProviderToken>("providerToken")) ?? null;

    if (
      this.providerToken !== null &&
      this.providerToken.keyId === this.env.APNS_KEY_ID &&
      now - this.providerToken.issuedAt < PROVIDER_TOKEN_LIFETIME_MS
    ) {
      return this.providerToken.value;
    }

    const value = await signProviderToken(
      this.env.APNS_KEY!,
      this.env.APNS_KEY_ID!,
      this.env.APNS_TEAM_ID!,
      Math.floor(now / 1000),
    );

    this.providerToken = { value, issuedAt: now, keyId: this.env.APNS_KEY_ID! };
    await this.ctx.storage.put("providerToken", this.providerToken);

    return value;
  }
}

/// An APNs provider token: an ES256 JWT naming the key and the team.
/// WebCrypto's ECDSA signature is the raw `r || s` pair JWS expects.
async function signProviderToken(pem: string, keyId: string, teamId: string, issuedAt: number): Promise<string> {
  const body = pem.replace(/-----(BEGIN|END) PRIVATE KEY-----/g, "").replace(/\s+/g, "");
  const der = Uint8Array.from(atob(body), (char) => char.charCodeAt(0));

  const key = await crypto.subtle.importKey("pkcs8", der, { name: "ECDSA", namedCurve: "P-256" }, false, ["sign"]);

  const header = base64Url(new TextEncoder().encode(JSON.stringify({ alg: "ES256", kid: keyId })));
  const claims = base64Url(new TextEncoder().encode(JSON.stringify({ iss: teamId, iat: issuedAt })));
  const input = `${header}.${claims}`;

  const signature = await crypto.subtle.sign(
    { name: "ECDSA", hash: "SHA-256" },
    key,
    new TextEncoder().encode(input),
  );

  return `${input}.${base64Url(new Uint8Array(signature))}`;
}

function base64Url(bytes: Uint8Array): string {
  let binary = "";

  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }

  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
