# iOS App Design

Status: proposal, 2026-09-27. Builds on the remote sessions protocol in
`docs/research/remote-sessions-design.md` (cited below as RS §n) and on what
M1-M6 shipped.

## 1. Summary

A native iOS app that pairs with a NiumaTerm desktop host through the user's
own Cloudflare relay and drives the host's terminal and agent sessions. The
protocol, cryptography, terminal engine, and agent view model come from the
desktop crates, compiled into one Rust library the Swift app links. Swift
owns only the platform: windows, input, rendering, notifications, keychain.

| Area | Decision |
| --- | --- |
| UI | SwiftUI, with UIKit where SwiftUI has no control (terminal surface, text input with marked text, edit menus). Deployment target iOS 26 now, iOS 18 later (§11). |
| Shared code | `crates/mobile` (`nmt_mobile`), a UniFFI library built as `NiumaTermCore.xcframework`. It links `nmt_remote_core`, the client half of `nmt_remote`, the engine half of `nmt_terminal`, and the model half of `nmt_agent`. |
| Transport | Relay only in v1. The same Noise IK channel as desktop clients. |
| Pairing | Scan a QR code of the existing pairing link (RS §6.4), shown by the desktop; typed code plus relay URL and access key as fallback. |
| Terminal | libghostty-vt engine in the core, fed the host checkpoint and live output exactly as a desktop client is. Swift draws the core's cell grid with Core Text. |
| Agent | The host keeps the controller. The core holds the replicated `AgentView` and sends `AgentCommand`s. SwiftUI renders the transcript and slots. |
| Push | The host seals each notification for the phone. The relay signs an APNs request and forwards the sealed bytes. A Notification Service Extension opens them on the phone (§9). |

## 2. Goals and non-goals

Goals for v1:

- Pair an iPhone or iPad with a desktop host through the relay, by QR code.
- List every host session, open new terminals and agents in the host's
  workspaces, attach to running ones, and take over control from the desktop
  (the one-controller model of RS §21).
- Terminal: a full VT client (TUIs, colors, scrollback, selection, links,
  IME input, hardware keyboards).
- Agent: everything a remote desktop pane can do today (§8.4).
- Push notifications when an agent finishes a turn, needs an approval, or
  asks a question, readable on the lock screen.
- Reuse desktop code for everything that is not UI or OS integration.

Non-goals for v1:

- LAN connections and DNS-SD discovery (feasible later, §12).
- Local sessions on the phone. iOS cannot spawn a shell or an agent CLI.
- Hosting sessions from the phone.
- Android. The core is portable, but nothing here targets it yet.
- An App Store release for other people. v1 ships to the user's own devices
  through Xcode and TestFlight under the user's developer account (§9.8
  explains what a public release would change).

## 3. Architecture

```text
 iPhone / iPad                                         Desktop host
+-----------------------------------------------+     +----------------------+
| SwiftUI app                                   |     | NiumaTerm (host)     |
|   Hosts & sessions   Terminal view  Agent view|     |   HostService        |
|        |                  |              |    |     |   SessionRegistry    |
|        v                  v              v    |     |   Push sealing (new) |
| NiumaTermCore.xcframework (Rust, UniFFI)      |     +----------+-----------+
|   MobileCore: store, keys, connections        |                |
|   TerminalHandle: libghostty-vt engine        |   Noise IK     |
|   AgentHandle: AgentView replica, commands    |<==(relay)=====>+
+-----------------------------------------------+                |
| Notification Service Extension                |                | POST /v1/push
|   opens sealed payloads (CryptoKit)           |                v
+-----------------------------------------------+     +----------------------+
         ^                                            | Relay Worker          |
         |               APNs (HTTP/2)                |   HostRoom (existing) |
         +--------------------------------------------+   Push route (new)    |
                                                      +----------------------+
```

Principles:

1. Rust owns every byte that crosses the network and every state machine that
   the desktop already tests. Swift never parses a protocol frame.
2. Swift owns everything the user touches. The core hands it plain data:
   cell rows, transcript entries, slot values, outcomes.
3. A behavior the desktop and the phone both need is written once, in the
   desktop crate that already has it, behind a Cargo feature if it drags in
   desktop-only dependencies.

## 4. Code reuse

### 4.1 Crate by crate

| Crate | Reused for | Change needed |
| --- | --- | --- |
| `nmt_remote_core` | Identity, pairing (SPAKE2 + Noise XXpsk3), pairing link parsing, IK channel, frames, RPC types | None. It is sans-IO and was written for this. |
| `nmt_remote` | `client` (dial, pair, relay path), `connection` (`RemoteHost`: reconnect, streams, `ended` map, `keep_connected`), `store` (paired hosts JSON), `secret` | Features: `host` (host service, session registry, `local_view`), `lan` (mdns-sd, LAN dialing), `terminal-pty` (`NetworkPty`). The mobile core uses `default-features = false`. `secret` gains an iOS keychain backend; `store` takes its directory from the caller; `netwatch` takes network change events from Swift (`NWPathMonitor`). |
| `nmt_terminal` | libghostty-vt engine, render state, grid, selection, links, input encoding (legacy and kitty keyboard), mode tracking | Feature `pty` gating process spawning, termio host paths, and the `copypasta` clipboard. The engine must run with terminal responses off, as desktop clients already do. |
| `libghostty-vt-sys` | The VT engine | `build.rs` learns `aarch64-apple-ios` and `aarch64-apple-ios-sim`. Zig cross-compiles both; the prebuilt cache gets iOS entries. |
| `nmt_agent` | `chat` types, `transcript`, `session::view` (`AgentView`, `ViewOp`, slots), `session::command` (command parameters and outcomes), `branch::BranchPicker`, question types | Feature `backends` gating the provider modules (`claude_code`, `codex`, `dsh`, `team`, `update`, `subprocess`, `background_task`), `reqwest`, and `which`. The mobile core uses only the model. |
| `tree_sitter_bundle` | Syntax highlighting of code blocks and diffs in agent transcripts | Optional in v1; include if it builds for iOS without changes. |
| `markdown` (markdown-rs, workspace dependency) | Parsing agent messages | None. The core turns mdast into a small block tree for Swift (§8.3). |
| `assets/i18n/*.toml` | Strings shared with the desktop (session states, agent notices, error texts) | A script generates an Xcode String Catalog from the keys the app uses. |
| `assets/themes`, terminal palettes | Terminal colors and the app accent | The core exposes the resolved palette; Swift does not parse theme files. |

What stays in Swift: views, navigation, gestures, text input, drawing,
keychain access groups for the extension, APNs registration, notification
handling, photo picking, haptics.

### 4.2 The mobile core crate

`crates/mobile` (`nmt_mobile`), `crate-type = ["staticlib"]`, UniFFI
proc-macro bindings. It owns one small tokio runtime (two workers) and exposes
three objects:

```rust
#[derive(uniffi::Object)]
pub struct MobileCore { /* store, device key, RemoteHost per host */ }

#[uniffi::export]
impl MobileCore {
    #[uniffi::constructor]
    pub fn new(state_dir: String, keychain: Arc<dyn Keychain>) -> Arc<Self>;

    pub async fn pair(&self, link_or_code: String, relay: Option<RelayInput>) -> Result<HostRecord, CoreError>;
    pub fn hosts(&self) -> Vec<HostRecord>;
    pub fn forget(&self, host: String);
    pub fn observe(&self, observer: Arc<dyn CoreObserver>);   // status, sessions, ended
    pub fn network_changed(&self);                            // from NWPathMonitor
    pub fn set_foreground(&self, foreground: bool);           // connect / wind down

    pub async fn host_info(&self, host: String) -> Result<HostInfo, CoreError>;
    pub async fn open_terminal(&self, host: String, profile: Option<String>, cwd: Option<String>) -> Result<String, CoreError>;
    pub async fn open_agent(&self, host: String, profile: String, workspace: String) -> Result<String, CoreError>;
    pub fn attach_terminal(&self, host: String, session: String, observer: Arc<dyn TerminalObserver>) -> Arc<TerminalHandle>;
    pub fn attach_agent(&self, host: String, session: String, observer: Arc<dyn AgentObserver>) -> Arc<AgentHandle>;
    pub async fn register_push(&self, token: Vec<u8>, environment: PushEnvironment) -> Result<(), CoreError>;
}
```

- Callbacks (`CoreObserver`, `TerminalObserver`, `AgentObserver`) are UniFFI
  foreign traits. The core calls them from its runtime threads; Swift hops to
  the main actor. Terminal callbacks only say "frame dirty"; Swift pulls the
  frame on its next display link tick, so a flood never queues work on the
  main thread.
- Records crossing the boundary are flat and render-ready. The core converts
  `chat::Item` and friends into UniFFI records once, in `nmt_mobile`, so the
  desktop types do not grow FFI attributes.
- The same crate can serve Android later through UniFFI's Kotlin bindings.

### 4.3 Build

- `scripts/build-ios-core.sh` (macOS): `cargo build --release` for
  `aarch64-apple-ios` and `aarch64-apple-ios-sim`, `uniffi-bindgen` for the
  Swift file, `xcodebuild -create-xcframework`. The Xcode project runs it as a
  pre-build phase when the Rust sources are newer than the framework.
- Swift package `NiumaTermCore` wraps the xcframework and the generated Swift,
  so the app and future extensions import one module.
- Release builds strip symbols and use `lto = "thin"`; the target is a core
  under 25 MB before App Store thinning.

## 5. Project layout

```text
crates/mobile/                  nmt_mobile (Rust, UniFFI)
mobile/ios/
  project.yml                   XcodeGen spec (the .xcodeproj is generated)
  NiumaTerm/                    app target
    App/                        entry, scene phases, deep links
    Hosts/                      host list, pairing, host detail
    Terminal/                   TerminalSurface (UIKit), accessory bar
    Agent/                      transcript, composer, sheets
    Settings/
    Compat/                     availability wrappers (§11)
  NotificationService/          Notification Service Extension
  Packages/NiumaTermCore/       xcframework + generated bindings
scripts/build-ios-core.sh
```

Identifiers: bundle `<team prefix>.niumaterm`, extension
`<bundle>.notification-service`, App Group `group.<bundle>`, keychain access
group `<team id>.<bundle>.shared`.

## 6. Connection and app lifecycle

- One channel per paired host, as on desktop (RS §8.3), relay path only.
  `nmt_remote::connection` already reconnects with backoff, resumes streams,
  and reports status; the phone reuses it unchanged.
- Foreground: the core connects to every paired host and keeps them
  connected while the host list is visible (`keep_connected`, as the desktop
  sidebar does), so session lists stay live.
- Background: iOS suspends the app within seconds. On `scenePhase ==
  .background` the app asks for a background task (about 30 s) so a command
  just sent can finish, then the core closes its channels. Sessions keep
  running on the host; push notifications cover the time away.
- Return to foreground: channels reconnect, terminals reattach with a fresh
  checkpoint, agents reattach with a snapshot. Both are the existing resync
  paths. The terminal checkpoint needs the `scrollback_rows` bound that RS §21
  lists as missing, so a phone reattaching to a long session does not
  download its whole history (§13).
- `NWPathMonitor` reports path changes to `network_changed`, which triggers
  the same immediate retry and link probe the desktop does on Windows.
- A host that is offline shows "Offline" in the list. The relay's 4404 close
  code (host not answering) maps to that state instead of an error.

## 7. Pairing

### 7.1 Phone side

- "Add computer" opens a camera scanner (`DataScannerViewController` from
  VisionKit, available since iOS 16) that accepts `niumaterm://pair?...` QR
  codes. The link carries code, host id, host key, relay URL, and relay
  access key (RS §6.4), so nothing is typed.
- The app also registers the `niumaterm` URL scheme, so scanning the QR code
  with the system Camera app opens the app on the pairing screen.
- Fallback form: code, relay URL, relay access key. The core runs the same
  pairing through `/v1/pair/{slot}`.
- The phone presents `DeviceInfo { kind: "mobile", platform: "ios" }`.
- On success the phone shows the host's name and device id; the desktop shows
  the phone in its paired devices list, where it can be removed.
- The LAN addresses in the link are ignored in v1.

### 7.2 Desktop side

- The pairing row on Settings › Remote › This computer shows a QR code of
  `pairing_link()` next to the code and the existing "Copy link" button. The
  `qrcode` crate renders a module matrix; the settings page paints it as
  squares, so no image codec is involved.
- The QR code appears only while relay hosting is configured, because the v1
  phone cannot use a link without a relay. Without one the row explains that
  the phone needs the relay.
- The QR code carries the relay access key. It is shown only while the code
  is active (5 minutes, one use) and disappears as soon as pairing succeeds.

## 8. Sessions

### 8.1 Navigation

- iPhone: `NavigationStack`. Root: paired hosts, each a section listing its
  sessions (terminals, then agents) with title, kind icon, agent activity
  (working, waiting for approval, idle), and a "controlled on desktop" mark.
  A toolbar "+" per host offers New terminal and each profile × workspace
  pair from `host.info`, like the desktop sidebar's host menu.
- iPad: `NavigationSplitView` with the same list as the sidebar and the
  session as detail. Stage Manager and external displays need nothing extra.
- Settings: paired hosts (status, Forget with confirmation), notifications
  per event kind, terminal font and size, accessory keys, theme, about.

### 8.2 Terminal

Data path: `TerminalHandle` wraps a core-side `TerminalSession` fed by the
connection's stream events (the logic of `NetworkPty`). After each batch it
marks the frame dirty. Swift pulls:

```rust
pub struct TerminalFrame {
    pub cols: u16, pub rows: u16,
    pub cursor: CursorState,          // position, shape, visible, blinking
    pub rows_changed: Vec<RowUpdate>, // index + style runs
    pub scrollback: ScrollState,      // offset, total rows
    pub selection: Option<SelectionRange>,
    pub title: Option<String>,
    pub modes: TerminalModes,         // mouse tracking, bracketed paste, app cursor keys
}
```

Rendering:

- `TerminalSurface` is a single `UIView` drawn with Core Text. Each pulled
  frame invalidates only the rects of the rows it changed, and the display
  link pulls at most one frame per refresh, so a flood costs at most one
  redraw per frame.
- One layer per row does not pay off. The engine's row versions are keyed
  by screen position, so scrolling by one line, the most common change
  (`cat`, logs, build output), changes every row, and full-screen programs
  repaint the whole grid anyway. A per-row cache would only help if keyed by
  content, with a layer pool and layers moved on scroll, which costs
  nearly as much code as a GPU renderer without its payoff. Layer count and
  memory are not the concern: about 50 row layers take the same backing
  store as one full-screen layer.
- A full redraw is about 2000 cells (roughly 55 columns by 25 to 50 rows in
  portrait). If measurement on a device shows dropped frames during floods
  or at 120 Hz, the surface moves to a `CAMetalLayer` with a glyph atlas:
  each glyph is rasterized once and every frame redraws all cells as quads,
  which keeps frame time flat no matter how much of the grid changed. The
  frame records from the core stay the same for both renderers.
- Wide characters, emoji, box drawing, and powerline glyphs follow the
  engine's cell widths; box drawing is drawn as paths so it joins cleanly.
- Font: JetBrains Mono, bundled as its Nerd Font Mono build (SIL OFL), so
  powerline separators and prompt icons draw without a second font. It is
  tall and open at small sizes, which matters on a phone grid. CJK text falls
  back to PingFang SC through a Core Text cascade list, one CJK character
  taking two cells as the engine reports. SF Mono is offered as an
  alternative in Settings. Pinch zooms.

Input:

- A hidden view that adopts `UITextInput` receives software keyboard text,
  including marked text from Chinese and Japanese IMEs. Marked text is drawn
  at the cursor and sent only when committed.
- An accessory bar above the keyboard: Esc, Tab, sticky Ctrl and Alt, arrows,
  `|`, `~`, `/`, `-`, Home/End, PgUp/PgDn, paste, and a keyboard dismiss key.
  The layout is user-editable in Settings.
- Hardware keyboards arrive as `UIPress` events. The core's key encoder turns
  them into bytes honoring application cursor mode and the kitty keyboard
  protocol, the same code the desktop uses.
- Paste uses bracketed paste when the program asked for it. `UIPasteControl`
  avoids the system paste prompt.

Gestures:

- Vertical pan scrolls the scrollback, or sends wheel events when the program
  enabled mouse tracking (vim, htop, less with mouse).
- Tap sends a click when mouse tracking is on; otherwise it focuses input.
- Long press starts a selection with handles; `UIEditMenuInteraction` offers
  Copy, Paste, Select All, and Open Link.
- Tap on a URL opens it in Safari (with confirmation). Host file paths offer
  Copy only.
- Pinch changes the font size, which changes the grid and claims the PTY size.

Size: the phone controls the session while it has it, so its grid decides
the PTY size (portrait gives about 45-55 columns). A per-session toggle "Keep
desktop width" keeps the host's columns and lets the grid pan horizontally;
it depends on the cropping fix listed in RS §21 (M3 gaps).

Other: bell plays a haptic; OSC 52 writes go to `UIPasteboard` behind the
same policy setting the desktop has; the title updates the navigation title;
exit shows the exit code and a Close button.

### 8.3 Agent

Data path: `AgentHandle` attaches with `agent.attach`, applies SNAPSHOT and
`agent.ops` to its `AgentView`, and reports which transcript range and which
slots changed. Commands go out as `agent.call` with the existing
`AgentCommand` parameter types, and their outcomes come back typed.

The replica is a plain `AgentView` plus an `apply(ViewOp)` function, placed in
`nmt_agent::session::view` next to `ViewPublisher`. That keeps the publisher
and its inverse in one file, and one test (publisher output applied to an
empty view equals a fresh snapshot) guards both desktop and phone. The phone
does not need the replica `SessionController` the desktop uses, because it
renders from the view directly.

Transcript, rendered in a SwiftUI `List` with stable ids per entry:

| Item | Presentation |
| --- | --- |
| User message | Bubble with images (fetched once through the `image` command and cached by id) |
| Agent message | Markdown: the core parses with markdown-rs and hands Swift blocks (heading, paragraph with inline runs, list, quote, code with language, table, rule). Code blocks scroll horizontally and have Copy. |
| Reasoning | Collapsed row, expands to the summary |
| Command execution | Work row with purpose or command, status and exit code; expands to output in a monospace view |
| File change | Paths and status; tap opens a diff viewer (unified, colored by line kind) |
| Compaction | Divider with the summary behind a disclosure |
| Other tool | Kind and title, expandable output |
| Error | Inline error row |

Consecutive work items fold into one "Show work (n)" row, as on desktop.
Streaming replies re-render only the last entry, because the core reports
the splice start.

Composer and controls:

- Multiline text field, Send and Interrupt buttons driven by the `status`
  slot, images from `PhotosPicker` or the camera (sent the same way desktop
  remote panes send them).
- `/` opens the command palette from the `catalogs` slot; skills likewise.
  Slash commands the host refuses for remote panes are hidden.
- Model, effort, approval preset, agent preset, and plan mode pickers from
  the `settings` slot.
- Queued prompts from the `queue` slot, each withdrawable.
- Status line: phase, elapsed time, output tokens, context usage from the
  `status` and `usage` slots. Tasks and goal show in a sheet.
- Approvals: a bottom sheet with the request and Approve / Deny, driven by
  the `pending` slot. Questions: a sheet with the question batch, options,
  free text, and the mode rules, sending drafts and answers as desktop does.
- Rewind and fork: the `branch` slot drives a picker sheet; each choice is a
  `branch` step. `/resume` shows the `history` slot as a list.
- Rename from the navigation title menu.

### 8.4 Control handover

The phone follows the one-controller model of RS §21 unchanged. Attaching
from the phone takes control: the desktop tab shows its frosted sheet naming
the phone, while its content stays live. When the desktop user switches back,
the phone receives `session.ended { reason: taken_back }` and shows a sheet
with Reconnect and Close. `reason: closed` leaves only Close, which removes
the session from the phone's list without touching the host.

## 9. Push notifications

### 9.1 Events

| Event | Host trigger | Title / body |
| --- | --- | --- |
| Turn finished | `SessionEffect::TurnCompleted` without error | "<Agent> finished · <session title>" / the latest agent message, first 180 characters |
| Turn failed | `TurnCompleted` with error | "<Agent> stopped with an error" / the error |
| Approval needed | `SessionEffect::ApprovalRequested` | "<Agent> needs approval" / the request description |
| Question | `SessionEffect::InputRequested` | "<Agent> asks" / the first question |

These are the effects `publish_activity` already turns into desktop
notifications, so the host hooks push in at the same place and reuses the
same texts. Terminal sessions do not push in v1 (§12).

Whether a phone hears about them depends on its presence, which the host
keeps per paired device, in memory only:

| Presence | Entered when | Pushes |
| --- | --- | --- |
| Paired | Just paired; the host started; the person used the host (took a session back, or brought a NiumaTerm window to the front); disconnected for 12 hours | No |
| Connected | A channel is open, or closed less than 5 minutes ago | No: the phone shows it on screen |
| Disconnected | Connected before, and no channel for 5 minutes | Yes |

The grace keeps a quick switch to another app from collecting pushes; the
12 hours end pushes for a session left running and forgotten. A suspended
phone stops answering the channel's liveness probes, so its channel closes
about 50 seconds after iOS suspends the app, and pushes start about six
minutes after the phone was put away. The Settings › Remote page lists each
device's presence.

### 9.2 Keys and registration

- The phone creates a random 32-byte push key per host and stores it in the
  shared keychain access group, where the extension can read it.
- After APNs registration and on every connect the phone sends a new request,
  `push.register { endpoint, token, environment: "sandbox" | "production", key,
  kinds }`, over the encrypted channel. `endpoint` is the forwarder the app
  build names (§9.4), and `kinds` the events left on in Settings. The host
  stores them in that device's trust store record (a new optional field, so no
  schema bump) and sends a push only to devices that registered. Both requests
  answer `{}`: a `null` result reads as a missing one to released clients.
- `push.unregister` removes them when the user turns notifications off;
  forgetting the host on the phone or removing the phone on the desktop drops
  them too.
- Both methods sit behind a new `push` feature in the hellos and a minor
  protocol bump (RS §9.5).

### 9.3 Sealing

The host encrypts a small JSON body with ChaCha20-Poly1305 under the device's
push key and a random 96-bit nonce:

```json
{ "v": 1, "host": "<host id>", "session": "<session id>", "kind": "turn_finished",
  "title": "Claude finished · Fix login", "body": "Updated the token refresh...", "at": 1790000000000 }
```

The host id is the associated data, so a sealed body cannot be replayed
under another host's name. Sealed form: `base64(nonce || ciphertext || tag)`,
at most 3 KB so the APNs
4 KB limit holds with the `aps` dictionary. The phone opens it with CryptoKit
`ChaChaPoly`, which exists on every supported iOS version. The format is
tested with vectors generated by the Rust side.

### 9.4 Relay route

The host hands each sealed push to the forwarder the phone registered, which
signs the APNs request and sends it. As built (2026-09-28), the forwarder is
an endpoint of the relay Worker, enabled only on the deployment that holds
the APNs key:

```text
POST /v1/push
{ "token": "<hex>", "environment": "production", "host": "<host id>",
  "sealed": "<base64>", "collapse": "<session id>" }
```

- The app goes to friends through TestFlight under one developer account, and
  only that account's key can sign its pushes. So one relay forwards pushes
  for every host the app pairs with, including hosts that use other relays,
  and the endpoint takes no access key. It stays narrow instead: the topic is
  a Worker secret, not a request field, so it reaches only this app; payloads
  are ciphertext; and each device token gets 60 pushes a minute.
- One `PushGateway` Durable Object signs and sends every push. It signs an
  APNs provider JWT (ES256 over the `.p8` key, WebCrypto `importKey("pkcs8")`
  and `sign`) carrying the key id and team id, keeps it in storage with the
  key id that signed it, and reuses it for 50 minutes: APNs rejects tokens
  older than an hour and throttles ones refreshed more often than every 20
  minutes, which separate isolates signing their own would trigger.
- It sends `POST https://api.push.apple.com/3/device/<token>` (or
  `api.sandbox.push.apple.com` for development builds) with `apns-topic`,
  `apns-push-type: alert`, `apns-collapse-id: <session id>`,
  `apns-priority: 10`, and:

```json
{ "aps": { "alert": { "title": "NiumaTerm", "body": "Agent update" },
           "mutable-content": 1, "sound": "default", "thread-id": "<session id>" },
  "h": "<host id>", "s": "<sealed>" }
```

  `h` is copied from the path, since the phone needs it to pick the key
  before anything is decrypted.
- The APNs status and reason go back to the host in the response.
  `410 Unregistered` and `400 BadDeviceToken` make the host drop that token.
  Other failures are logged and not retried; a missed push is replaced by the
  session state the phone sees when it next opens.
- New secrets: `APNS_KEY` (the `.p8` contents), `APNS_KEY_ID`,
  `APNS_TEAM_ID`, `APNS_TOPIC`, set with `wrangler secret put`. Without them
  the route answers `501`. The relay README has a section on creating the key
  in the developer account.
- Rate limit: 60 pushes per host per minute.
- Cost: one Worker request and one Durable Object request per push, against
  the free plan's 100,000 of each per day. JWT signing happens once per 50
  minutes and stays far inside the 10 ms CPU limit; waiting on APNs is not
  CPU time.

Why the relay: every host already talks to it, so the Apple key is set up
once for all of the user's desktops, hosts need no Apple configuration, and
the key sits in one place the user already runs as their own
infrastructure.

Caveats:

- APNs accepts HTTP/2 only. A deployed Worker's outbound `fetch` negotiates
  HTTP/2 (open-source Worker APNs clients depend on it), but Cloudflare does
  not document the protocol version of subrequests, so the route rests on
  observed behavior. Confirmed on 2026-09-28: a deployed Worker's pushes
  reached a simulator through the sandbox, and made-up tokens drew
  `BadDeviceToken` from both environments.
- Local `wrangler dev` cannot reach APNs, because the local runtime does not
  speak HTTP/2 outbound. Push tests run against a deployed relay; everything
  else keeps using `wrangler dev`.
- The relay now holds a secret beyond the access key. It still holds nothing
  that can read channels or notification text.

Fallback: if Cloudflare stops negotiating HTTP/2, the same request moves
into the host. The desktop already ships `reqwest`, which speaks HTTP/2 once
its `http2` feature is on; the host would sign the JWT with `p256` and keep
the `.p8` key in its secret storage. §9.2, §9.3, and §9.5 stay the same, so
the phone does not change.

### 9.5 On the phone

- The Notification Service Extension reads `h` and `s`, looks up the push
  key for host `h`, opens `s` with `h` as associated data, and replaces
  title and body. On failure (key gone, bad data) it leaves
  the generic text, so a push is never lost, only less specific.
- `threadIdentifier` groups notifications per session. `userInfo` carries
  host and session for routing.
- Tapping a notification opens the app on that session, connecting and
  attaching as needed.
- While the app is in the foreground, `willPresent` hides banners for the
  session on screen and shows the others.
- Lock-screen previews follow the system "Show Previews" setting.

### 9.6 Categories and actions

v1 registers categories without actions. Approve / Deny from the
notification is feasible later: the action wakes the app in the background,
which connects, sends `respond_approval`, and finishes within the action's
time budget. It needs `authenticationRequired` so a locked phone cannot
approve.

### 9.7 Later: Live Activities

A Live Activity per running agent turn (phase, elapsed time, last tool) can
be driven by ActivityKit push tokens through the same relay route with
`apns-push-type: liveactivity`. It is left for after v1.

### 9.8 Distribution note

The APNs key belongs to the developer account that signs the app. That works
while the user builds the app for their own devices. If the app were ever
published for other people, self-deployed relays could not hold the
publisher's key, and pushes would need a push forwarder run by the publisher
(the same route, deployed once by the publisher). Sealing already keeps
notification text away from such a forwarder, as it does from the user's
relay and from Apple today.

## 10. Security

- Device key: generated in the core, stored in the keychain with
  `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`, never synced to
  iCloud, never exported. Removing the phone on the desktop revokes it.
- The relay access key arrives inside the QR code or the pairing exchange
  and is stored like the device key.
- Push keys live in the shared access group so the extension can read them;
  the extension gets nothing else.
- The relay and Apple see device tokens, host ids, timing, and sizes, never
  notification text.
- The `.p8` key can send pushes to any install of the user's apps. It is a
  Worker secret, readable only by the Worker, and the push route answers
  only hosts presenting their relay token.
- A malicious host gets the protections desktop clients have (RS §14):
  terminal responses off, file-based image transmissions refused, host paths
  never opened, OSC 52 under the phone's policy.
- Face ID lock for the app: an optional setting that requires device
  authentication when returning from the background.

## 11. iOS 26 now, iOS 18 later

The deployment target starts at 26. Nothing in the design needs iOS 26 for
function; iOS 26 is used for looks and newer SwiftUI conveniences. To keep
the later move to 18 a matter of changing the target:

- Every iOS 26-only API goes through `Compat/` wrappers with an iOS 18
  fallback from day one, even while the fallback cannot run:

| iOS 26 API | Use | iOS 18 fallback |
| --- | --- | --- |
| Liquid Glass (`glassEffect`, `GlassEffectContainer`, glass button styles) | Accessory bar, composer, floating controls | `.ultraThinMaterial` backgrounds, bordered buttons |
| `ToolbarSpacer`, new toolbar grouping | Session toolbars | Plain toolbar items |
| Tab bar minimize and bottom accessory | Not used; navigation is a stack and a split view | n/a |
| SwiftUI rich `TextEditor` with `AttributedString` | Not needed; the composer is plain text | n/a |
| `@Observable` and `NavigationStack` | Everywhere | Available since iOS 17; no fallback needed |

- The core and the extension use nothing newer than iOS 16 (CryptoKit,
  VisionKit scanner, `UIEditMenuInteraction`, `UIPasteControl`).
- CI builds a second configuration with `IPHONEOS_DEPLOYMENT_TARGET=18.0`
  and fails on unguarded availability errors, so regressions show before the
  switch.

## 12. What the phone cannot do

| Desktop capability | iOS | Reason |
| --- | --- | --- |
| Local terminal and agent sessions | Not possible | iOS forbids spawning shells and CLI processes; App Review rejects downloaded executables. |
| Hosting sessions for other devices | Not possible | Follows from the row above, and the app cannot keep listening in the background. |
| Live output while the app is in the background | Not possible | iOS suspends background sockets. The phone catches up from a checkpoint or snapshot on return; pushes cover agent events. |
| Push for terminal events (bell, command finished) | Later | Possible with the same route; "command finished" needs block metadata the checkpoint does not carry for history. |
| LAN direct connection | Later | Needs the Local Network permission and Bonjour declarations; excluded from v1 by scope, not by the platform. |
| Split panes, several sessions visible at once | iPad later; iPhone no | Screen size. iPad can show two sessions in split view once the single-session screen is stable. |
| Mouse hover, right-click, precise drag in TUIs | Partial | Touch maps tap to click and pan to wheel; iPad trackpads and mice deliver real pointer events. |
| Every keyboard shortcut | Partial | Software keyboards lack function keys and combinations; the accessory bar covers common ones, hardware keyboards cover the rest. |
| Command block chrome and the block list | Later | Desktop block rendering lives in the GPUI app; the phone shows the plain grid first. |
| Terminal images (kitty graphics, sixel) | Later | The engine decodes them; drawing placements is extra work. Images placed before attaching are not re-sent (RS §10.5). |
| Opening host file paths, reveal in folder, open in editor | Not possible | Paths name the host's disk. The phone offers Copy. |
| Drag and drop of files, non-image attachments | Not possible in v1 | No host file browser in the protocol (`fs.list` not offered); images work. |
| Agent side questions, background tasks, workflow agents, conversation search, new conversation | Not possible until the host supports them | Host-only today for every remote pane (RS §21, M6 gaps). |
| History pages beyond the first | Not possible until the host supports it | Same M6 gap. |
| Team sessions | Not possible in v1 | The remote protocol does not expose team orchestration. |
| Agent profiles, credentials, hooks, agent CLI updates | Not possible | Host-only by design (RS §11.6). |
| Token usage and quota panels of the desktop sidebar | Later | Needs a new `host.info` field. |
| Approve or deny from a notification | Later | Feasible (§9.6). |
| Restoring the cut prompt after a remote rewind or fork | Not possible until the host supports it | Same gap as desktop remote panes. |
| Keeping desktop width while the phone controls | Later | Needs the cropping fix from RS §21 (M3 gaps). |

## 13. Changes outside the app

Desktop and protocol:

1. Cargo features: `nmt_remote` (`host`, `lan`, `terminal-pty`),
   `nmt_terminal` (`pty`), `nmt_agent` (`backends`). Desktop builds keep all
   of them on, so desktop behavior does not change.
2. `libghostty-vt-sys` iOS targets and prebuilt entries.
3. `ViewReplica::apply` in `nmt_agent::session::view` with its round-trip
   test.
4. `terminal.attach { scrollback_rows }` and the bounded checkpoint
   (RS §10.2), used by the phone with 2000 rows.
5. `sessions.list` entries gain an optional `activity` field (working,
   waiting for approval, waiting for an answer, idle) so the phone's list can
   show agent state without attaching.
6. `push` feature, `push.register` and `push.unregister`, push fields in the
   trust store record, the push sender hooked into `publish_activity`, and a
   minor protocol bump.
7. QR code on Settings › Remote › This computer (§7.2).
8. The phone shows as `kind: mobile` in the desktop's paired devices list with
   a phone icon.

Relay:

9. `POST /v1/push/{host_id}` with APNs signing, the three secrets, the rate
   limit, and the README section (§9.4).

## 14. Testing

- Core, on desktop CI: `nmt_mobile` against an in-process host over loopback
  (the existing `loopback_tests` harness): pair through a pairing link,
  attach a terminal and compare its frame with the host engine, attach an
  agent and check replica equality after scripted controller events, push
  registration and sealing vectors.
- Core, on macOS CI: build the xcframework for device and simulator; run the
  Rust tests on the simulator target.
- Swift: unit tests for the view models (transcript grouping, slot-driven
  controls, accessory key encoding requests), snapshot tests for transcript
  rows.
- End to end: a desktop `--testing` host with relay hosting against
  `wrangler dev`, the app in the simulator paired by pasting the link. Covers
  opening, attaching, control handover, reconnect after toggling the
  simulator's network, and background/foreground cycles.
- Push: a development build on a real device with the sandbox APNs
  environment, including a failure case (deleted push key) that must still
  show the generic notification.

## 15. Delivery plan

| Phase | Scope | Done when |
| --- | --- | --- |
| P0 | Spikes: the core builds for iOS with libghostty-vt; UniFFI async calls work from Swift; a deployed relay sends a sandbox APNs push | A simulator app prints a host's session list through the relay, and a push sent through the deployed relay's route arrives on a device. |
| P1 | Cargo feature splits, `nmt_mobile` skeleton, keychain, pairing by QR and form, host list with live sessions, desktop QR code | The phone pairs by QR and lists sessions that update as the desktop opens and closes tabs. |
| P2 | Terminal: surface, input with IME, accessory bar, hardware keyboard, gestures, selection, resize, reconnect, `scrollback_rows` | vim, htop, and a Claude Code TUI are usable from the phone; typing Chinese with the system IME works; the session survives backgrounding. |
| P3 | Agent: replica, transcript, markdown, composer, pickers, approvals, questions, branch and history sheets, images, control handover | A Claude, Codex, and DeepSeek session is driven from the phone, including an approval, a question, a fork, and an image prompt. |
| P4 | Push: registration, host sealing and sending, relay route, extension, routing | With the app in the background, finishing a turn shows the agent's reply on the lock screen, and tapping it opens that session. |
| P5 | iPad split view, settings, Face ID lock, iOS 18 build configuration, TestFlight | TestFlight build installed on iPhone and iPad; the iOS 18 configuration builds. |

## 16. Decisions

Settled on 2026-09-27:

1. Pushes go through the relay, which holds the APNs key; the host-side
   sender stays the documented fallback (§9.4).
2. Attaching from the phone takes control of the session, as a desktop
   client does (§8.4). No watch-only mode in v1.
3. The terminal font is JetBrains Mono Nerd Font Mono with PingFang SC for
   CJK (§8.2).
