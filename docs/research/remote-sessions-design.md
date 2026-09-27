# Remote Sessions Design

Status: proposal, 2026-09-26. This design starts from scratch. The earlier
relay-only terminal feature was removed in `b27d5cfa` and nothing here depends
on it.

## 1. Summary

A NiumaTerm instance can host its terminal and agent sessions for paired
devices. Another NiumaTerm instance (and later a mobile app) pairs once with a
short one-time code, then connects over the local network or through a relay.
Every connection, on every path, runs the same mutually authenticated,
end-to-end encrypted channel, so neither the LAN nor the relay is trusted.
Inside the channel a small multiplexed protocol carries RPCs, terminal byte
streams with checkpoints, and agent view state as snapshots plus ordered
operations.

| Area | Decision |
| --- | --- |
| Trust | One X25519 static key per installation. The host keeps a list of paired device keys and revokes them individually. |
| Pairing | 8-character one-time code. SPAKE2 turns it into a strong key; `Noise_XXpsk3` exchanges static keys under that key. |
| Channel | `Noise_IK_25519_ChaChaPoly_BLAKE2s` on every connection, LAN or relay. |
| LAN | WebSocket over TCP, discovered through DNS-SD. |
| Internet | User-deployed Cloudflare Worker + Durable Object relay that forwards opaque frames. |
| Terminal | The host PTY and host VT engine stay authoritative. The client runs its own engine, fed a VT checkpoint followed by the live byte stream. |
| Agent | The host owns the provider process and `SessionController`. Clients render a replicated view (snapshot + revisioned operations) and send typed commands. |
| Mobile | A sans-IO protocol core crate that iOS and Android can link. Pairing link, device kinds, and feature negotiation are defined now. |

## 2. Goals and non-goals

Goals for v1:

- From computer B, open terminals and agent sessions that run on computer A,
  and attach to sessions already running on A.
- LAN and Internet paths, chosen automatically, with identical security.
- Pair once; later connections need no user action; revoke per device.
- Sessions keep running on the host across network loss. Clients reconnect and
  resume without user action.
- A protocol a mobile client can implement without desktop-only assumptions.

Non-goals for v1:

- The mobile app, push notifications, QR rendering. Their formats are
  specified here; implementation waits for the app.
- Linux. v1 hosts and clients are Windows and macOS; Linux later needs its own
  secret storage (Secret Service) and firewall notes.
- A relay operated by the project. Users deploy their own (§8.2).
- Direct Internet peer-to-peer (UDP hole punching). Non-LAN traffic uses the
  relay.
- A headless host daemon. The host is a running NiumaTerm application.
- Permission levels. Every paired device is a full operator. The pairing record
  carries a `role` field so a read-only role can be added without migration.
- File transfer, port forwarding, remote file editing.
- Moving a live channel between LAN and relay. A path change is a reconnect.

## 3. Terms

- **Device**: one NiumaTerm installation (or future mobile install) with its own
  static key.
- **Host**: a device exposing its sessions. **Client**: a device using them. One
  device can be both.
- **Pairing**: the one-time procedure that stores each side's public key on the
  other.
- **Channel**: one authenticated, encrypted connection between a client and a
  host.
- **Session**: a terminal or agent session owned by the host. It outlives
  channels and views.
- **View**: anything rendering a session: a host tab, a client tab, later a
  phone screen.
- **Stream**: a numbered flow inside a channel carrying one view's data for one
  session.

## 4. Architecture

```text
 Client device (NiumaTerm B)                        Host device (NiumaTerm A)
+-----------------------------+                   +--------------------------------+
| Remote terminal tab         |                   | Host service (tokio)           |
|   TerminalFrameSource       |                   |   LAN listener + DNS-SD        |
|     NetworkPty ----------+  |                   |   Relay host link              |
| Remote agent tab         |  |                   |   Trust store                  |
|   AgentView replica -----+  |                   |   Channel handlers             |
| Connection manager <-----+  |   Noise channel   |          | requests, streams   |
|   LAN dialer (DNS-SD) ------+--- TCP/WebSocket -->         v                     |
|   Relay dialer -------------+--> Relay -------->| Session registries (UI thread) |
+-----------------------------+   (opaque bytes)  |   TerminalSession + PTY        |
                                                  |   AgentSession + controller    |
                                                  +--------------------------------+
```

Principles:

1. Sessions belong to the host. Channels and views come and go.
2. One secure channel for every transport. A transport only moves ordered
   binary messages.
3. The relay forwards opaque bytes and holds no keys.
4. State converges instead of being logged. A terminal can always be rebuilt
   from a checkpoint and an agent view from a snapshot. Streams exist for
   latency; snapshots exist for correctness.
5. A client renders remote sessions with the same code it uses for local ones.

## 5. Identity and trust

### 5.1 Device key

- An X25519 static key pair is generated on first use of the remote feature.
- Device id: the first 10 bytes of SHA-256(public key) in lowercase Crockford
  base32, 16 characters, displayed as `abcd-efgh-jkmn-pqrs`. It names the
  device in the UI and logs, and a host's id is its relay routing key.
- Private key storage: Windows DPAPI (`CryptProtectData`, current-user scope)
  through the existing `windows` crate; macOS Keychain generic password through
  `security-framework` (already in the lock file). The binary-embedded AES key
  used for agent profile credentials is not acceptable here: whoever reads this
  key can run commands on every host that trusts it.
- `--testing` instances use their own state directory, therefore their own
  identity and trust store. Two test instances on one machine can pair with
  each other without touching the user's real pairing records.

### 5.2 Trust store

Host side, one record per paired device:

```json
{
  "schema": 1,
  "id": "k7q2m9xd4fab3c1e",
  "name": "Work laptop",
  "kind": "desktop",
  "platform": "windows",
  "public_key": "<base64>",
  "role": "operator",
  "paired_at": 1790000000000,
  "last_seen": 1790000500000,
  "last_hello_ms": 1790000500000
}
```

Client side, one record per paired host:

```json
{
  "schema": 1,
  "id": "m3n8p2q7r4s9t1v5",
  "name": "Studio PC",
  "public_key": "<base64>",
  "relay": "wss://relay.example.com",
  "lan_hints": ["192.168.1.20:47470"],
  "paired_at": 1790000000000,
  "last_seen": 1790000500000
}
```

Both live as JSON in the application state directory, not in the user config
TOML, because pairing creates them rather than hand editing. Public keys are not
secret. Anyone able to rewrite the file already runs as the user.

Removing a record closes that device's channels immediately and rejects its
later handshakes.

A bearer token (paseo's QR payload, a password) is deliberately not used for
access. It is copyable, shared by every client, and cannot be revoked for one
device without re-pairing all of them. A device key never leaves its device's
secret storage. The only secret a user ever handles is the one-time pairing
code.

## 6. Pairing

### 6.1 Code

- 8 symbols of Crockford base32 (`0-9`, `A-Z` without `I L O U`), shown as
  `K7Q2-M9XD`. Input ignores case and separators and maps `I`/`L` to `1` and
  `O` to `0`.
- Symbols 1-3 are the rendezvous slot (15 bits). Symbols 4-8 are the secret
  (25 bits). The slot is independent random data, so exposing it to the LAN or
  the relay reveals nothing about the secret.
- Lifetime: 5 minutes, one successful use, invalidated after 3 failed attempts.
  The host dialog shows the countdown and can cancel.

25 bits suffice only because the code feeds a PAKE. An attacker, including a
malicious relay, can test one guess per attempt against the live host and none
offline. Three attempts give a success chance below 1 in 10 million per code.
The same code used directly as a Noise PSK would fall to an offline search in
seconds.

### 6.2 Rendezvous

The client finds the host from the slot, trying every available route
concurrently; the first route that completes pairing wins.

- LAN: while a code is active the host's DNS-SD record carries `pair=<slot>`.
  The client browses `_niumaterm._tcp` for the matching record.
- Relay: the host claims the slot on its relay for the code's lifetime. The
  client resolves the slot to the host id and connects like any client. Since
  the relay is the user's own, a client that has never paired knows no relay:
  the host's pairing dialog shows "relay: <URL>" and the client's dialog has an
  optional relay field (prefilled when the client already knows one). Pairing
  with a relay whose access key the client lacks uses the pairing link (§6.4),
  which carries the key.
- Manual: the client enters the address shown in the host's pairing dialog, for
  networks that block multicast or for VPNs such as Tailscale.

### 6.3 Exchange

A connection starts with the versioned preface of §9.5 (kind `0x01` channel,
`0x02` pairing).

```text
C -> H  PairHello  { v: 1, slot, spake: A }                plaintext
H -> C  PairReply  { spake: B }                           plaintext
        both sides: K = SPAKE2(code, A, B), 32 bytes
        identities "niumaterm-pair-client" / "niumaterm-pair-host"
C -> H  Noise_XXpsk3 msg1: e
H -> C  msg2: e, ee, s, es                                 empty payload
C -> H  msg3: s, se, psk(K)                                DeviceInfo(client)
        H: PSK check fails -> count the attempt, close
        H: success -> store the client key, invalidate the code
H -> C  transport message: PairAccepted { host: DeviceInfo, relay, lan_hints }
        C: store the host
```

- `DeviceInfo = { name, kind: "desktop" | "mobile", platform, app_version }`.
- Host details travel only after the PSK check, so a wrong-code attempt learns
  nothing beyond the host's public key.
- Both sides then show "Paired with <name>" and the peer's device id.
- The client closes the pairing connection and opens a normal channel (§7).
  Pairing never grants a session on its own.

### 6.4 Pairing link

Reserved for mobile; desktop v1 accepts it when pasted but renders no QR code.

```text
niumaterm://pair?v=1&c=K7Q2M9XD&h=<host id>&k=<host public key, base64url>
                &r=<relay URL>&rk=<relay access key>&a=<ip:port>,<ip:port>
```

The link runs the same PAKE with the same code. The client additionally checks
that the host key it learns equals `k`, and connects straight to the listed
addresses and relay without a slot lookup. A QR code is this link.

## 7. Secure channel

- Pattern `Noise_IK_25519_ChaChaPoly_BLAKE2s` through the `snow` crate.
  Prologue: `NiumaTerm remote` followed by the exact preface bytes (§9.5), so
  both sides bind the major version they agreed on and a relay that rewrites
  the preface makes the handshake fail.
- msg1 (client): `e, es, s, ss` with payload
  `ClientHello { proto_minor, app_version, features, hello_ms }`.
- The host decrypts msg1 and looks the client key up in its trust store. An
  unknown or revoked key closes the socket without a reply. A `hello_ms` not
  greater than that device's `last_hello_ms` is a replayed msg1 and is dropped;
  otherwise the new value is stored.
- msg2 (host): `e, ee, se` with payload
  `HostHello { proto_minor, app_version, features, name, lan_hints }`. The
  hints let a client that came in through the relay find the host on the LAN
  next time.
- A client sends `hello_ms = max(now_ms, last_sent + 1)` so a clock stepping
  backwards cannot lock it out.
- msg1 never carries application data: it is replayable and not forward secret.
- Transport messages use Noise counter nonces. Both transports are reliable and
  ordered, so any modified, reordered, duplicated, or dropped frame fails
  decryption, which closes the channel.
- Result: mutual authentication, ciphertext-only visibility for the LAN and the
  relay, forward secrecy from `ee`, and the client identity hidden from passive
  observers.
- Handshake timeout 10 s. No rekeying in v1: nonces are 64-bit, and channels
  reconnect on every network change anyway.

## 8. Transports

Each transport delivers an ordered sequence of binary messages. One message is
one Noise message of at most 65535 bytes.

### 8.1 LAN

- Listener: TCP on all interfaces, IPv4 and IPv6, default port 47470
  (configurable; an ephemeral port when busy), WebSocket upgrade at `/v1`. One
  WebSocket binary message carries one Noise message.
- WebSocket rather than raw TCP: the relay path is WebSocket, so both paths
  share one framing; browsers and mobile platforms speak it natively;
  `tokio-tungstenite` is already a dependency.
- Discovery: DNS-SD service `_niumaterm._tcp.local.` through `mdns-sd`. The
  instance name is the device name; TXT records `v=1`, `id=<device id>`, plus
  `pair=<slot>` while a code is active. iOS and Android browse DNS-SD natively
  (NWBrowser, NsdManager), whereas custom UDP broadcast needs a restricted
  multicast entitlement on iOS.
- Exposure limits: at most 16 unauthenticated sockets, each closed if the
  handshake is not complete within 5 s.
- Enabling LAN hosting triggers the OS firewall prompt; the settings page says
  so before enabling.
- The DNS-SD record reveals the device name and id to the local network. Users
  on untrusted networks can keep LAN hosting off and use the relay.

### 8.2 Relay

A Cloudflare Worker with Durable Objects under `relay/`. The project ships the
source and a deployment guide but runs no relay: each user deploys it to their
own Cloudflare account (`wrangler deploy`) and enters its URL and access key on
the host. There is no default URL; without one, relay hosting stays off and
only LAN works.

Relay access key: a random secret set with `wrangler secret put ACCESS_KEY`.
Every host and client socket presents it (`Authorization: Bearer`), so a
stranger who learns the URL cannot use the user's relay as a free pipe on the
user's bill. The host hands URL and key to clients inside the encrypted pairing
exchange (`PairAccepted`), so clients never type them. The key guards the
account quota only; confidentiality never depends on it.

Objects:

- `HostRoom`, one per host id (`idFromName(host_id)`): the host control socket,
  client sockets, and host data sockets, all accepted through the WebSocket
  Hibernation API with tags (`control`, `c:<conn>`, `h:<conn>`). It stores
  SHA-256 of the host's relay token.
- `PairingDirectory`, a single instance: slot to host id, each entry expiring
  with its code.

Endpoints:

| Endpoint | Caller | Behavior |
| --- | --- | --- |
| `GET /v1/host/{host_id}` (WebSocket) | host | Control socket. Access key plus `X-Host-Token: <relay token>`. |
| `GET /v1/host/{host_id}/accept/{conn}` (WebSocket) | host | Data socket for one client connection, same credentials. |
| `GET /v1/client/{host_id}` (WebSocket) | client | New client connection. Access key. |
| `GET /v1/slot/{slot}` | client | `{ "host_id": ... }` or 404. Access key. |

Flow:

1. The host keeps the control socket open while hosting through the relay.
2. A client connects; `HostRoom` assigns a connection id and sends the control
   socket the text frame `{"t":"conn","id":"<conn>"}`.
3. The host opens the matching accept socket. `HostRoom` then sends the client
   the text frame `{"t":"open"}` and forwards binary frames between the two
   sockets without reading them. Closing either side closes the other.
4. The client starts its handshake only after `open`, so the relay never
   buffers payload. A client that sees no `open` within 10 s gets close code
   4404 (host offline or not answering).
5. Pairing slots are claimed and released over the control socket
   (`{"t":"slot","slot":"K7Q","ttl":300}`, answered `slot_ok` or
   `slot_taken`; the host picks another slot when taken).

Host ownership: the host generates a random 32-byte relay token and keeps it in
secret storage beside its key. The first control connection for a host id
stores the token hash (trust on first use); later host connections must present
the same token. Squatting would require learning the host id before the host's
first registration, and the host id is only disclosed through pairing, which
happens after registration.

Liveness and cost:

- Text `ping` every 30 s, answered by `setWebSocketAutoResponse` without waking
  the object or billing duration.
- Incoming WebSocket messages bill at 20:1 and outgoing ones are free; output
  coalescing (§9.3) keeps a busy terminal to a few hundred messages per second
  at worst. Personal use of two computers fits the Workers free plan (100,000
  Durable Object requests per day, 20 incoming messages counting as one).

Limits: 8 client connections per host, 128 KiB per message, 30 connection
attempts per minute per IP. The worst outcome of relay abuse is lost
availability, never lost confidentiality.

Operational notes:

- A relay deploy drops all sockets. Hosts and clients reconnect on their own,
  and sessions survive on the host.
- `*.workers.dev` is unreliable from some networks (for example mainland
  China). The deployment guide recommends a custom domain, and the protocol is
  small enough for a self-hosted Rust relay on any server later.
- Redeploying the Worker is the only relay maintenance; it has no state beyond
  token hashes and short-lived slots.

### 8.3 Connection strategy

- Candidates: cached LAN hints, DNS-SD results for the host id, and the relay.
- LAN candidates start at once. The relay starts after 300 ms, or immediately
  when every LAN candidate has failed. The first completed handshake wins and
  the rest are cancelled.
- Reconnect with exponential backoff from 0.5 s to 30 s with jitter, and retry
  at once when the OS reports a network change or a remote tab gains focus.
- One channel per host, shared by every remote tab for that host. It opens on
  demand and closes 60 s after the last remote tab for that host closes.

## 9. Session protocol

### 9.1 Frames

Every Noise plaintext is one frame:

```text
 0         4      5       6
 +---------+------+-------+--------------------------+
 | stream  | type | flags | payload (<= 65513 bytes) |
 | u32 LE  |  u8  |  u8   |                          |
 +---------+------+-------+--------------------------+
 flags bit 0 (MORE): the message continues in the next frame of this stream
```

- Stream 0 is the control stream; type `0x01` is a JSON message, and types
  `0x02` PING / `0x03` PONG are liveness probes the channel answers itself.
- The host allocates every other stream id in its attach or upload responses.
- Fragments of different streams may interleave. A reassembled message larger
  than 32 MiB closes the channel.

Stream frame types:

| Type | Direction | Payload |
| --- | --- | --- |
| `0x10` CHECKPOINT | host to client | VT bytes, fragmented, starts with a reset |
| `0x11` OUTPUT | host to client | raw PTY output |
| `0x12` INPUT | client to host | encoded terminal input |
| `0x13` EXIT | host to client | JSON `{ "code": 0 }` |
| `0x14` SIZE | host to client | JSON `{ "cols": 120, "rows": 40 }` |
| `0x20` SNAPSHOT | host to client | JSON agent view, fragmented |
| `0x21` OPS | host to client | JSON array of agent view operations |
| `0x30` BLOB | either | chunk of an upload or download |

### 9.2 Control messages

JSON-RPC 2.0 message shapes without batching. v1 only needs client-to-host
requests and host-to-client notifications. Error codes: `not_found`,
`invalid_params`, `unsupported`, `busy`, `denied`, `internal`.

| Method | Purpose |
| --- | --- |
| `host.info` | Name, platform, version, home directory, shell profiles, agent kinds and profile names (never credentials), recent workspaces. |
| `sessions.list` | Host sessions with kind, title, cwd, origin, and state. `subscribe: true` adds `sessions.changed` notifications. |
| `terminal.open` | Start a terminal from a host shell profile, cwd, and size. Returns the session id. |
| `terminal.attach` | Subscribe; returns a stream id and the PTY size, then CHECKPOINT and OUTPUT frames follow. |
| `terminal.resize` | Claim the PTY size (§10.4). |
| `terminal.close` | Terminate the session. |
| `agent.open` | Start an agent session: kind, profile name, workspace roots, optional conversation to resume. |
| `agent.attach` | Subscribe; optional `since_revision`. Returns a stream id, then SNAPSHOT or OPS frames follow. |
| `agent.*` commands | See §11.5. |
| `agent.close` | End the session. |
| `stream.close` | Detach any stream. |
| `fs.list`, `fs.complete` | Host directory listing and path completion for workspace pickers and `@` mentions. |
| `blob.put`, `blob.get` | Chunked upload and download by SHA-256, for images. |

Notifications: `sessions.changed`, `host.goodbye { reason }`.

### 9.3 Flow control

- Each side has one outbound queue per channel. The host bounds it at 8 MiB;
  crossing the bound closes the channel and the client reconnects.
- A terminal stream with more than 1 MiB pending drops that pending output. Once
  the queue drains below 256 KiB the host sends a fresh checkpoint (resync).
  Floods such as `cat` of a large file therefore degrade to skipped frames, not
  unbounded latency.
- An agent stream with more than 1 MiB pending drops its operations and resyncs
  with a snapshot.
- Coalescing: terminal output flushes within 5 ms or at 32 KiB; agent
  operations within 16 ms.
- Client-to-host input and commands are small and need no special handling.

### 9.4 Liveness

- The relay path pings at the relay level (§8.2).
- Either side sends a PING frame after 30 s without inbound frames. No answer
  within 10 s, twice in a row, marks the channel dead and starts a reconnect.
  Probes are frames rather than a `ping` request so the channel layer answers
  them without the request dispatch above it.

### 9.5 Versioning

Two peers can run different releases: one machine updates first, and a phone
app ships on a different schedule from the desktop. Every layer therefore
carries an explicit version, and the rules below say what each change costs.

#### Version numbers

| Layer | Carried in | Scope |
| --- | --- | --- |
| Protocol major | Connection preface, bound into the Noise prologue | Frame layout, handshake patterns, anything an old peer cannot skip safely |
| Protocol minor | `ClientHello` / `HostHello` (`proto_minor`) | Additive changes: new methods, fields, frame types, operations |
| Features | Hellos (`features: ["terminal", "agent", ...]`) | Optional capabilities a build may lack regardless of version (for example `agent` on a terminal-only mobile client) |
| Pairing exchange | Preface kind `0x02` with its own major | SPAKE2 and pairing messages |
| Pairing link | `v=` query parameter | Link fields |
| Relay protocol | URL prefix `/v1/` | Relay endpoints and text frames |
| Stored records | `"schema": 1` in trust store and paired-host files | On-disk format, migrated forward on load |
| Application | `app_version` in hellos | Display and diagnostics only; never used for behavior decisions |

#### Preface and major negotiation

The first transport message of every connection is cleartext:

```text
 "NMTR" | kind u8 | major u8 | min_major u8
```

- `major` is the highest major the sender speaks, `min_major` the lowest.
- The host picks the highest major both ranges share and answers with the same
  preface layout carrying that single value (`major = min_major = chosen`),
  then the handshake runs under that major.
- Without a shared major the host answers `"NMTR" | 0xFF | major | min_major`
  and closes. The client then states which side is too old: "Studio PC runs an
  older remote protocol; update NiumaTerm there" or the reverse.
- A build keeps the previous major for at least one release cycle after
  introducing a new one, so hosts and clients can update in either order.
- The preface is inside the Noise prologue, so tampering with it to force an
  older major fails the handshake.

#### Minor negotiation

- Each hello carries `proto_minor`. Both sides use `min(own, peer)` as the
  effective minor for the channel and never emit anything newer.
- Every method, field, frame type, stream type, agent operation, and enum value
  records the minor that introduced it (a `since` column in the protocol
  tables). The sender checks the effective minor before emitting it.
- Optional capabilities use `features` instead of the minor, because a newer
  build may still leave them out.

#### Change rules

| Change | Needs |
| --- | --- |
| New optional JSON field | Nothing; unknown fields are ignored |
| New method | Minor bump; older peers answer `unsupported` |
| New enum value, frame type, stream type, agent operation | Minor bump; emitted only when the effective minor allows it. Receivers still decode unknown enum values into an `Unknown` case and skip unknown frame types, as a safety net |
| Removing or renaming anything, changing a field's meaning or type, changing frame layout or cryptography | Major bump |
| Optional capability | Feature name |

Fields and methods are never removed within a major; they are only marked
deprecated. `nmt_remote_core` holds the version constants, and a test fixture
of recorded messages per minor fails when a change alters how an older
message decodes.

## 10. Terminal sessions

### 10.1 Model

```text
Host:    PTY <-> PTY loop: host engine (answers queries) -> subscribers
                                                             | CHECKPOINT, OUTPUT
Client:  NetworkPty -> TerminalSession: client engine (no replies) -> TerminalFrameSource -> view
```

Shipping VT bytes instead of rendered cells keeps every client feature working
unchanged (selection, search, scrollback, links, blocks for new commands),
because the client runs a full engine. It also keeps the host free of
per-client rendering, and it matches what a mobile client would do with the
same engine or any other VT emulator.

### 10.2 Host side

- A host-wide terminal registry lists tab-owned and headless sessions. Each
  entry exposes a `Send` control handle (a clone of the PTY loop's message
  sender plus shared flags) so the network runtime writes input and resizes
  without passing through the UI thread.
- The PTY loop message `Subscribe { sink, checkpoint }` formats a checkpoint
  and registers the sink in the same loop step. Every later byte reaches the
  sink in engine order, so nothing is lost or duplicated between checkpoint and
  live output. A sink returning `false` unsubscribes. `Checkpoint` alone serves
  resync for a sink that stays subscribed, since its completion is ordered with
  the bytes the sink receives.
- Checkpoint bytes: `ESC c`, `CSI 3 J`, the finished blocks (see below),
  then the formatter's VT output with
  every `extra` flag enabled (palette, modes, scrolling region, tabstops, pwd,
  keyboard, cursor, style, hyperlink, protection, kitty keyboard, charsets).
  `format_vt_state` emits all of these. While the alternate screen is active
  it writes the primary screen and its cursor first, then the alternate-screen
  modes, then the alternate screen, so a replica keeps the shell's content and
  scrollback once the full-screen program exits.
- In block mode each finished command is frozen out of the screen the
  formatter reads, so the checkpoint first writes every block as styled VT
  (patch 0007), then line feeds that scroll it into history. The replica gets
  that history as plain scrollback.
- Checkpoints carry all history. A row bound for resync and phones
  (`scrollback_rows`, 2000 rows) waits for those callers.
- `terminal.open` accepts a host shell profile name, cwd, and size, not an
  arbitrary executable and environment. A paired device can run anything by
  typing it; the restriction keeps the API small, it is not a security
  boundary.
- Sessions a remote device created run headless on the host and ignore host-side
  events (clipboard writes, notifications, bell): no host user is looking at
  them. They appear in the host's remote panel, where the host user can close
  them or open a local view.

### 10.3 Client side

- `NetworkPty` implements `nmt_platform::AsyncPty`: `poll_read` yields
  CHECKPOINT then OUTPUT bytes, `poll_write` sends INPUT frames, `poll_resize`
  sends `terminal.resize`, and `poll_exit` completes on EXIT or when the session
  is gone for good. While the channel reconnects, reads stay pending and the
  tab shows a reconnecting overlay.
- `TerminalFrameSource::attach`, currently test-only, becomes the production
  entry point: remote tabs build `TerminalSession::from_pty(NetworkPty, ...)`.
- The client engine runs with terminal responses disabled. The host engine,
  next to the PTY, answers DA, DSR, and OSC queries, so the program gets exactly
  one reply without a network round trip.
- Kitty graphics transmissions naming a file, temporary file, or shared memory
  are refused, because those paths would be read from the client's disk at the
  request of the remote side. libghostty-vt engines accept only direct
  transmissions unless configured otherwise, and NiumaTerm never widens that.
- A CHECKPOINT arriving mid-stream starts with a reset. The reset clears the
  screen and history but not the client engine's own finished blocks, so the
  client clears those before applying a resync checkpoint.
- File paths in links refer to the host. v1 offers copying them, not opening.
- OSC 52 clipboard writes follow the client's existing clipboard policy: a
  program on the host can set the clipboard of the machine the user sits at,
  as with SSH.

### 10.4 Size

The last active view wins. Typing into, focusing, or resizing a view claims the
PTY size. Other views receive `SIZE` and size their engine to the PTY, cropping
or padding the display. Attaching, redrawing, or merely being visible never
claims.

### 10.5 Limitations in v1

- Block metadata for history (command, exit code, duration) is not
  reconstructed; commands run after attaching get full block chrome.
- Images placed before attaching are not re-sent.

## 11. Agent sessions

### 11.1 Where to split

Three cut points were considered:

| Cut | Client runs | Verdict |
| --- | --- | --- |
| Provider process | Backend adapters and controller; host only runs the CLI | Rejected: adapters, hooks, credentials, and transcript files are host-local. |
| `chat::Event` stream | Controller and pane; host runs `Backend` | Rejected: history, checkpoint, child, and workflow reads target host files and would each need a proxy; two controllers on one backend diverge, so only one view could exist; reconnect has to rebuild controller state from events. |
| Controller output | Pane only; host runs `Backend` and `SessionController` | Chosen. |

The chosen cut keeps every file read on the host, allows any number of views,
turns reconnect into snapshot-plus-operations, and gives a mobile client a
render-only job. It builds on the existing ownership split in
`agent_tab/execution`: `AgentSession` owners live in a registry, exist without a
renderer, and panes attach to them. What is missing is a serializable view of
the controller state and a command path that does not borrow the controller.

### 11.2 View model

The view is what a pane reads today, as data:

- Transcript: ordered entries `{ turn, item, at_ms, images }`. `item` mirrors
  `chat::Item`; images are blob references `{ sha256, media_type, len }`.
- Slots, each replaced whole when it changes:
  `status` (runtime phase, turn timing, output tokens, retry, compaction),
  `settings` (model, effort, presets, plan mode, available models),
  `catalogs` (slash commands, skills), `pending` (approval request, questions),
  `queue`, `goal`, `tasks`, `usage` (context window, composition, session
  stats), `children`, `workflows`, `title`, `recovery` (conversation identity
  for resuming).

Durations and timestamps are host-computed milliseconds, so client clocks do
not matter.

### 11.3 Operations

Each OPS frame carries operations tagged with consecutive revisions:

```json
[
  { "rev": 41, "op": "slot", "key": "status", "value": { "phase": "working" } },
  { "rev": 42, "op": "splice", "from": 17, "entries": [ { "turn": 6, "item": {} } ] },
  { "rev": 43, "op": "append", "index": 17, "field": "text", "text": "partial answer" },
  { "rev": 44, "op": "notice", "kind": "effort_rejected", "message": "..." }
]
```

- `splice` replaces every entry from `from` to the end. It covers pushes,
  completions, rewinds, forks, and clears with one rule and maps onto the
  existing `ContentChange { first, .. }` revision log.
- `append` carries streamed text for agent messages, reasoning summaries, and
  command output.
- `notice` carries transient events that are not state (a refused setting, a
  slash command result). Snapshots never contain notices.

The host produces operations after each processed event batch by comparing slot
versions and the transcript's first changed index. A gap in `rev` makes the
client drop its replica and request a snapshot.

### 11.4 Attach and resume

`agent.attach { session, since_revision? }` returns operations after
`since_revision` if the host still holds them (a log of the last 512
operations), otherwise a SNAPSHOT at the current revision. A reconnecting
client passes its last revision and usually receives only what it missed.

### 11.5 Commands

Each command is a method returning the domain outcome the local path already
produces (for example `SendOutcome`), never a UI reaction:

| Method | Maps to |
| --- | --- |
| `agent.submit` | Submit text, skill, settings, image blob references |
| `agent.interrupt` | Interrupt the running turn |
| `agent.respond_approval` | Approve or deny the pending request |
| `agent.answer_question` | Answer or dismiss a question |
| `agent.run_command` | Slash command with arguments |
| `agent.set_settings` | Model, effort, approval or agent preset, plan mode |
| `agent.withdraw_queued` | Remove a queued prompt |
| `agent.rewind`, `agent.fork` | Branch operations at a checkpoint |
| `agent.rename` | Rename the conversation |
| `agent.history`, `agent.search`, `agent.resume` | List, search, and continue past conversations of the workspace |
| `agent.open_child`, `agent.interrupt_child` | Background task details and control |
| `agent.open_workflow_agent` | Workflow agent transcript |
| `agent.side_question` | Ask a side question |

Child and workflow transcripts use their own attach streams with the same
snapshot and operation rules, and the host keeps refreshing them only while a
view is attached, like the existing reader-interest rule.

### 11.6 Files, images, credentials

- Images a client attaches are uploaded with `blob.put` and referenced by hash;
  the host stores them where the local path stores composer images.
- Transcript images are fetched lazily with `blob.get` and cached by hash.
- Workspace pickers and `@` mentions use `fs.list` and `fs.complete`; every
  path is a host path.
- Profiles, API credentials, hooks, agent binaries, and their updates stay on
  the host. Clients see profile names only.

### 11.7 Several views of one session

- Drafts, scroll position, folding, selection, and layout are per view and
  never sent.
- Approvals and questions are shared: the first answer wins and every view sees
  the `pending` slot clear.
- Submissions from different views are serialised by the host session in
  arrival order, exactly as queued or steered prompts are today.
- A host with no local view of a remote-created session shows no notifications
  for it.

### 11.8 Integration steps

1. Add `AgentView` (the data in §11.2) and `AgentCommand` to `nmt_agent`, with
   the host projection from `SessionController`.
2. Route the pane's direct controller mutations (about 40 `borrow_mut` sites in
   `agent_tab/mod.rs`) through commands. The local command path executes them
   synchronously against the owner, so this step changes no behavior and the
   existing UI session tests cover it.
3. Make the pane read through one accessor type that borrows the owner's
   controller for a local session and the replica for a remote one. The replica
   holds a real `ConversationState` so transcript rendering code is shared.
4. Add the network command path and the replica fed by SNAPSHOT and OPS.

Steps 1 to 3 are a pure refactor and can land before any networking.

## 12. Application integration

### 12.1 Threads

- The host service and the client connection manager run on the existing
  shared tokio runtime (`nmt_platform::runtime`).
- Sessions stay where they live today: agent controllers on the UI thread,
  terminal engines on their PTY loops.
- Requests that create, close, attach, or command sessions reach the UI thread
  through a channel with one-shot replies. Terminal output goes from the PTY
  loop straight into channel queues, and terminal input goes from tokio straight
  to the PTY loop, so terminal latency never waits for a UI frame.
- Anything delivered from the network into a GPUI entity arrives outside a
  frame and must call `cx.notify`, not only queue a next-frame callback, or the
  update shows up only when an unrelated repaint happens.

### 12.2 Settings

New `[remote]` keys, appended to the config:

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | `false` | Host sessions for paired devices |
| `lan` | `true` | Accept LAN connections while hosting |
| `relay-url` | empty | The user's own relay; empty disables relay hosting |
| `lan-port` | `47470` | LAN listener port |
| `device-name` | computer name | Name shown to peers |

The relay access key and the relay token are secrets and live in secret
storage beside the device key, not in the TOML file.

Connecting to other hosts needs no setting; it is available whenever a host is
paired.

### 12.3 UX

Host:

- Settings, Remote page: hosting toggles, relay URL and access key (with a
  link to the deployment guide and a "Test" button), "Pair a device" (code,
  countdown, addresses and relay for manual entry), paired devices (name,
  kind, last seen, connected state, Remove), remote-created sessions (Close,
  Open here).
- Every host session, including terminals and agents already open in host
  tabs, is listed to paired devices and can be attached. A tab with remote
  viewers shows a small indicator naming the attached devices.
- Status bar indicator with the number of connected devices.

Client:

- "Connect to a computer…" dialog: code, optional address, progress, result.
- Sidebar section of paired hosts with online state. Per host: New terminal,
  New agent (host workspace picker and host profiles), Sessions on this host.
- Remote tabs carry a host badge. Lost connection shows a reconnecting overlay
  and blocks input. An ended session shows its final state, and agent tabs offer
  "Resume conversation" through the `recovery` slot.
- Saved tabs store `{ host_id, session_id, kind }` and reattach on restore. A
  session that no longer exists shows the ended state.

On quit the host sends `host.goodbye` so clients show "host closed" instead of
reconnecting forever.

## 13. Mobile readiness

- `nmt_remote_core` is sans-IO: identity, pairing, channel, frames, protocol
  types, pairing link, with no tokio and no GPUI. It compiles for iOS and
  Android and can be exposed through UniFFI, so a mobile app reuses the exact
  cryptography and codecs instead of reimplementing them.
- Discovery uses DNS-SD, which both mobile platforms support natively.
- Terminal checkpoints are plain VT, so a phone can use libghostty-vt (C ABI) or
  any VT emulator. `scrollback_rows` keeps phone checkpoints small.
- The agent protocol is render-only by design (§11). A phone needs no provider
  adapter.
- Pairing records carry `kind` and `role`; a read-only viewer role can be added
  for phones without a migration.
- Reconnect is cheap (one round trip, then operations since a revision), which
  suits apps that the OS suspends.
- Push notifications, later: a phone registers its push token with the host
  inside the channel. The host sends notification payloads encrypted for that
  device through a relay endpoint that forwards them to APNs or FCM; a
  notification service extension decrypts them on the phone. The `push`
  feature flag reserves the negotiation.
- A phone is only useful when the host is reachable, which later calls for a
  "keep running in the background" host mode.

## 14. Security analysis

| Threat | Result |
| --- | --- |
| Relay or LAN reads traffic | Ciphertext only. |
| Relay modifies, replays, reorders, or drops frames | Decryption fails; channel closes. |
| Relay replays a handshake | `hello_ms` check drops it; msg1 carries no application data. |
| Pairing code brute force | Online only, one guess per attempt, 3 attempts per code. |
| Pairing code observed by someone nearby | Useful only within 5 minutes and before its single use; the host shows the new device and can remove it. |
| Unpaired device on the LAN or the relay | Rejected at msg1 without a reply. |
| Stolen client device | Remove it on the host. |
| Host static key stolen | The thief can impersonate the host to its clients until re-paired; recorded traffic stays confidential (forward secrecy). |
| Host id squatting on the relay | TOFU relay token. |
| Relay abuse or outage | Availability only; LAN keeps working. |
| Malicious host attacking a client | Terminal responses off on the client, file-based image transmissions refused, host paths never opened locally, OSC 52 under the client's policy. |
| Private key read from disk | DPAPI or Keychain. |
| Metadata | The relay sees IP addresses, timing, sizes, and host ids. DNS-SD reveals device names and ids on the LAN. |

## 15. Failure modes and limits

| Situation | Behavior |
| --- | --- |
| Network drop | Sessions keep running. The client reconnects with backoff, reattaches terminals with a checkpoint and agents from their last revision. |
| Host app quits | `host.goodbye`; sessions end. Agent tabs offer resuming the conversation after the host restarts. |
| Relay deploy | All relay sockets drop and reconnect. |
| Slow link or flood | Per-stream resync (§9.3); the channel closes only at the 8 MiB bound. |
| Version mismatch | Major: the preface exchange names the side to update. Minor: features negotiate down. |
| Code typo | Counts as one of the 3 attempts; the dialog says to check the code. |
| Clock skew | Only `hello_ms` ordering per device matters; clients never send a value lower than their last. |

## 16. Crates and files

| Path | Contents |
| --- | --- |
| `crates/remote_core` (`nmt_remote_core`) | Sans-IO: identity, pairing code and link, SPAKE2 + Noise pairing, Noise IK channel, frames, protocol types. |
| `crates/remote` (`nmt_remote`) | tokio: LAN listener and dialer, DNS-SD, relay links, host service, connection manager, trust store, secret storage. |
| `crates/terminal` | Subscriber list with atomic checkpoint, full-extras VT checkpoint, remote-session engine options. |
| `crates/agent` | `AgentView`, `AgentCommand`, projection. |
| `crates/app/src/remote/` | Settings page, pairing dialogs, sidebar hosts, `NetworkPty`, remote tabs, host-side session bridging. |
| `relay/` | Worker source, `wrangler.toml`; root `tsconfig.json` includes `relay/src`. |

New dependencies: `snow` (Noise), `spake2` (PAKE), `mdns-sd` (DNS-SD). Reused:
`tokio`, `tokio-tungstenite`, `sha2`, `data-encoding`, `serde_json`, `uuid`,
`windows` (DPAPI), `security-framework` (Keychain).

## 17. Testing

- `nmt_remote_core`: the right code pairs; a wrong code fails and is counted;
  the attempt limit invalidates the code; tampered, replayed, and reordered
  frames fail; a pinned host key, a wrong host, and a rewritten preface fail
  the handshake; frames fragment and reassemble at the limits.
- `nmt_remote`: an unknown device key and a replayed msg1 are rejected (both
  decisions need the trust store, so the core only exposes the client key and
  `hello_ms`); host and client over localhost WebSocket; relay flows against
  `wrangler dev`.
- Terminal: an engine fed a checkpoint renders the same as the engine that
  produced it (screen, cursor, modes, palette); subscribing during continuous
  output loses and duplicates no bytes (fake PTY harness).
- Agent: after any sequence of controller events, a replica built from the
  snapshot plus operations equals a fresh snapshot at the same revision.
- End to end: two `--testing` instances on one machine pair over loopback, open
  a remote terminal and a remote agent, survive a forced disconnect, and pair
  again through a local relay.

## 18. Delivery plan

| Milestone | Scope | Done when |
| --- | --- | --- |
| M1 | `nmt_remote_core` | Unit tests of §17 pass. |
| M2 | LAN transport, host service, trust store, settings page, pairing over LAN | Two test instances pair and hold a channel with pings. |
| M3 | Remote terminals: open, attach to host tabs, resync, reconnect, tab UX | A shell on host A is usable from B, including vim and a flood, across a cable pull. |
| M4 | Relay worker and deployment guide, relay links, pairing through the relay, path racing | M3 works with LAN hosting disabled. |
| M5 | Agent refactor steps 1-3 (no networking) | Existing agent tests pass unchanged. |
| M6 | Remote agents: attach, open, commands, blobs, file completion | A Claude, Codex, and DeepSeek session on A is driven from B, including approvals and images. |

## 19. Comparison with paseo

Taken from paseo: a relay that pairs a host control socket with per-client data
sockets and never parses payload; binary terminal frames beside JSON messages;
last-interacting-view-wins PTY size; append-only schemas with capability-gated
values; high-water marks that resync instead of buffering; live streams for
latency with authoritative snapshots for correctness.

Changed:

| paseo | NiumaTerm |
| --- | --- |
| Anyone holding the QR payload (the daemon public key) can connect; no per-device revocation | Mutual static-key authentication and a per-device trust store |
| NaCl `box` with random nonces: no protection against replay or reordering inside a session; no forward secrecy if the daemon key leaks | Noise IK: counter nonces, ephemeral key agreement |
| Direct connections are plaintext WebSocket with an optional password | Every path uses the same encrypted channel; LAN is just a transport |
| Pairing by QR or link only | Typed 8-character code with a PAKE, which suits two computers; the link and QR remain for phones |
| A separate daemon owns sessions | The application is the host; sessions outlive views inside it |

## 20. Decisions

Settled on 2026-09-26:

1. The relay is self-deployed by each user; the project ships source and a
   guide and operates none. Hence no default URL and a relay access key (§8.2).
2. Paired devices may attach to every host session, including tabs already
   open on the host. There is no toggle; removing a device is the control.
3. Linux is out of scope for v1.

## 21. Implementation status

Updated 2026-09-27.

| Milestone | State |
| --- | --- |
| M1 | Done: `nmt_remote_core` with identity, preface, pairing (SPAKE2 + `Noise_XXpsk3`), pairing link, IK channel, frames, control messages. |
| M2 | Done: LAN listener, trust store with DPAPI-sealed key, pairing over LAN, DNS-SD advertising and lookup (by slot and by device id), liveness probes, Remote settings page. |
| M3 | Done: session registry with host tabs, `sessions.list`/`sessions.changed`, attach to host tabs, per-stream flow control with resync, reconnect with backoff and reattach, `SIZE` frames with size reclaim on input, reconnecting banner, restore of remote tabs, checkpoints that carry the prompt lifecycle. |

Known gaps in M3:

- A view whose PTY another view resized renders the host's size on its
  own grid instead of cropping or padding; it takes the size back on its
  next input.
- Remote-created sessions can be closed from the host's settings page but
  not yet opened in a local tab ("Open here").
- Host tabs do not yet show which devices are attached, and there is no
  status bar count of connected devices.
- `scrollback_rows` is not implemented; checkpoints carry all history.
