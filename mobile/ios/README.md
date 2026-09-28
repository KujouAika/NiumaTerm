# NiumaTerm iOS

A SwiftUI app laid out as in `docs/research/ios-app-design.md` §5. The
protocol, cryptography, reconnects, and the agent view replica live in the
Rust core `crates/mobile` (`nmt_mobile`), which UniFFI exposes to Swift as the
`NiumaTermCore` module.

Status: pairing, the host and session list, and agent sessions (transcript,
send / queue / withdraw, interrupt, approvals, model and effort, rename, the
desktop taking a session back and the phone reconnecting) run on the real
core. Terminal sessions are the next milestone (P2): opening one shows a
placeholder, and the screens under `Terminal/` are still the prototype fed by
`Core/MockData.swift`.

## Build

1. Xcode 26 or later, and the Rust iOS targets:

   ```sh
   rustup target add aarch64-apple-ios aarch64-apple-ios-sim
   ```

2. Build the Rust core. This writes
   `Packages/NiumaTermCore/NiumaTermCoreFFI.xcframework` and the generated
   Swift bindings; neither is committed.

   ```sh
   scripts/build-ios-core.sh                # release, device + simulator
   scripts/build-ios-core.sh --debug --sim  # faster while developing
   ```

   Run it again after changing `crates/mobile`, `crates/remote`, or
   `crates/agent`.

3. Set your signing identity outside the project file, so it never reaches
   the repository:

   ```sh
   cp Config/Local.xcconfig.example Config/Local.xcconfig   # gitignored
   ```

   and fill in `NMT_DEVELOPMENT_TEAM` and `NMT_BUNDLE_ID`. Scripted builds
   can pass the same names as environment variables instead. Do not pick a
   team in Xcode's Signing & Capabilities tab: Xcode writes that choice into
   `project.pbxproj`. Without either, the app still builds for the simulator
   as `io.f32.NiumaTermMobile`.

4. Open `NiumaTerm.xcodeproj` and run.

## Testing against a local desktop host

1. Start an isolated desktop instance with remote hosting on, on a port apart
   from the one a running instance uses (47470):

   ```sh
   mkdir -p /tmp/nmt-host/Test
   cp ~/Library/Application\ Support/NiumaTerm/Test/config.toml /tmp/nmt-host/Test/
   printf '\n[remote]\nenabled = true\nlan-port = 47471\ndevice-name = "Test Host"\n' >> /tmp/nmt-host/Test/config.toml
   NMT_CONFIG_HOME=/tmp/nmt-host scripts/macos-dev-sign.sh target/debug/NiumaTerm --testing
   ```

2. On the desktop: Settings › Remote › This computer › Show pairing code ›
   Copy link.
3. In the simulator: `xcrun simctl openurl booted "$(pbpaste)"`, or Add
   computer › Paste pairing link in the app. A code works once, for five
   minutes.

Without a relay the app uses the LAN address in the pairing link; with one it
goes through the relay (design doc §7.1).

## Files

| Screen | File |
| --- | --- |
| Hosts and sessions | `Hosts/HostListView.swift` |
| New agent (profile × workspace from `host.info`) | `Hosts/NewSessionSheet.swift` |
| Pairing: scan, paste a link, or type a code | `Hosts/PairingView.swift` |
| Agent session and transcript | `Agent/AgentSessionView.swift`, `Agent/AgentSessionModel.swift` |
| Composer, model and effort | `Agent/ComposerView.swift` |
| Approval, session taken back or ended | `Agent/AgentSheets.swift` |
| Core callbacks onto the main actor | `Core/CoreEvents.swift` |
| App state over `MobileCore` | `App/AppModel.swift` |
| Terminal prototype | `Terminal/` |
| Liquid Glass with iOS 18 fallbacks | `Compat/Compat.swift` |

## Fonts

Drop the JetBrains Mono Nerd Font Mono `.ttf` files into
`NiumaTerm/Resources/Fonts/`; they are registered at launch. Without them the
app uses SF Mono.
