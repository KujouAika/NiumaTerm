# NiumaTerm iOS

A SwiftUI app laid out as in `docs/research/ios-app-design.md` §5. The
protocol, cryptography, reconnects, and the agent view replica live in the
Rust core `crates/mobile` (`nmt_mobile`), which UniFFI exposes to Swift as the
`NiumaTermCore` module.

Status: pairing, the host and session list, agent sessions (transcript,
send / queue / withdraw, interrupt, approvals, model and effort, rename, the
desktop taking a session back and the phone reconnecting), and terminal
sessions run on the real core. A terminal runs the desktop's libghostty-vt
engine in the core and is drawn by `Terminal/TerminalSurface.swift` with
Core Text, redrawing only the rows each frame changed (design doc §8.2).

## Build

1. Xcode 26 or later, the Rust iOS targets, and the Zig version the
   desktop build uses (the core compiles libghostty-vt for iOS with it):

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

   Run it again after changing `crates/mobile`, `crates/remote`,
   `crates/terminal`, or `crates/agent`.

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

## TestFlight

`scripts/ios-testflight.sh` builds the core for release, archives the app
with a build number taken from the time, and uploads it to App Store
Connect; `--no-upload` exports an `.ipa` instead. The bundle ID needs an
app record in App Store Connect first, and the first upload creates the
Apple Distribution certificate, so run it once signed in to Xcode with an
Admin or Account Holder Apple ID. Testers outside the team join through an
external group's public link; the first build of each version goes through
Beta App Review, whose reviewers cannot pair a computer, so the review
notes should say so.

`NiumaTerm/PrivacyInfo.xcprivacy` declares why the app uses the APIs App
Store Connect checks for. After adding a dependency, list the release
binary's imports (`nm -u`) and declare any new ones there.

## Files

| Screen | File |
| --- | --- |
| Hosts and sessions | `Hosts/HostListView.swift` |
| New terminal, or agent (profile × workspace from `host.info`) | `Hosts/NewSessionSheet.swift` |
| Pairing: scan, paste a link, or type a code | `Hosts/PairingView.swift` |
| Agent session and transcript | `Agent/AgentSessionView.swift`, `Agent/AgentSessionModel.swift` |
| Composer, model and effort | `Agent/ComposerView.swift` |
| Approval, session taken back or ended | `Agent/AgentSheets.swift` |
| Core callbacks onto the main actor | `Core/CoreEvents.swift` |
| App state over `MobileCore` | `App/AppModel.swift` |
| Terminal screen, accessory keys | `Terminal/TerminalSessionView.swift`, `Terminal/TerminalSessionModel.swift` |
| Terminal drawing and gestures | `Terminal/TerminalSurface.swift` |
| Keyboard, input methods, hardware keys | `Terminal/TerminalInput.swift` |
| Liquid Glass with iOS 18 fallbacks | `Compat/Compat.swift` |

## Fonts

Drop the JetBrains Mono Nerd Font Mono `.ttf` files into
`NiumaTerm/Resources/Fonts/`; they are registered at launch. Without them the
app uses SF Mono.
