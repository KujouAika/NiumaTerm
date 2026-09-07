# macOS Auto-Update via Sparkle

| Field | Value |
| --- | --- |
| Status | Research only; no code written |
| Date | 2026-09-06 |
| Scope | How the macOS build of NiumaTerm can self-update, and what Sparkle costs to adopt |
| Depends on | The `feature/macos` port reaching a launchable `crates/app` |
| Decided | Developer ID is available; the nightly channel ships on macOS too |

## 1. What exists today

The Windows updater lives in `crates/app/src/update/`. It reads the GitHub
Releases API directly (`releases.rs`), picks a release for the configured
channel, downloads a `.zip` plus its `.sha256` sidecar, unpacks into an
`update/` staging directory, uses the Restart Manager
(`crates/platform/src/windows/restart_manager/`) to find and close processes
holding the installed files, swaps them, and relaunches with `--await-exit`.

Two pieces of that are platform-neutral and worth keeping:

- **`nmt_version`** — the `v1.2.9` / `nightly-20260821-7567b41` label forms and
  their parsing. `crates/version/src/lib.rs` deliberately leaves "does this
  supersede that" to the caller, because the answer depends on the channel.
- **`nmt_config::update::UpdateConfig`** — the `[update] check-updates` and
  `[update] channel` settings, and the About-page controls in
  `crates/app/src/ui/settings/about_page.rs` that drive them.

Everything else — `Status`, `InstallError`, the staging swap, the file-user
prompt — is Restart Manager machinery with no macOS counterpart. The macOS side
should not try to reuse it.

There is also no `.app` bundle yet. `scripts/` is entirely PowerShell, and
`crates/app/build.rs` only stamps a Windows resource. A macOS build today
produces a bare Mach-O at `target/<profile>/NiumaTerm`.

## 2. Why Sparkle, and what it actually requires

Sparkle updates **app bundles**, not loose executables. Its installer replaces
`NiumaTerm.app` wholesale from a signed archive, releases the replacement from
quarantine, and relaunches. That removes the whole class of problem the Restart
Manager path exists to solve on Windows, because macOS lets a running
executable's bundle be replaced on disk.

Current release: **2.9.6** (2026-08-17). The `Sparkle-2.9.6.tar.xz`
distribution unpacks to:

```
Sparkle.framework/          3.0 MB, universal x86_64 + arm64, LC_BUILD_VERSION minos 11.0
  Versions/B/{Sparkle, Autoupdate, Updater.app, XPCServices, Resources, Headers}
bin/{generate_keys, sign_update, generate_appcast, BinaryDelta}
SampleAppcast.xml, CHANGELOG, LICENSE, INSTALL
```

Two properties of the shipped framework matter for a non-Xcode build:

- It is **ad-hoc signed** (`Signature=adhoc`, `TeamIdentifier=not set`). Every
  Mach-O inside it must be re-signed with the Developer ID identity before the
  outer app is signed — see §6.
- `XPCServices/` (`Downloader.xpc`, `Installer.xpc`, 424 KB combined) exists
  only for sandboxed hosts. A terminal emulator cannot be sandboxed, so the
  directory is deleted from the embedded copy. That also removes the
  `com.apple.security.temporary-exception.mach-lookup.global-name` entitlement
  pair (`-spks` / `-spki`) that sandboxed Sparkle hosts have to carry.

### Required Info.plist keys

| Key | Value |
| --- | --- |
| `SUFeedURL` | HTTPS URL of the appcast |
| `SUPublicEDKey` | base64 EdDSA public key from `bin/generate_keys` |
| `SUEnableAutomaticChecks` | `true` — suppresses Sparkle's own first-launch "check automatically?" prompt, which NiumaTerm already asks in its own settings |
| `SUScheduledCheckInterval` | seconds; the Windows path uses 6 h (`21600`), Sparkle's default is 24 h |
| `CFBundleVersion` | the machine-comparable version — see §4 |
| `CFBundleShortVersionString` | the label a user reads |

### Linking, from a Cargo build

Sparkle's own guidance for non-Xcode build systems is `-framework Sparkle`,
`-F<dir>`, `-Wl,-rpath,@loader_path/../Frameworks`, and a copy of
`Sparkle.framework` into `Contents/Frameworks/` with symlinks and executable
bits preserved.

In this workspace that lands as:

- a build script emitting
  `cargo:rustc-link-search=framework=<dir>` and
  `cargo:rustc-link-lib=framework=Sparkle`;
- `crates/app/build.rs` emitting
  `cargo:rustc-link-arg-bins=-Wl,-rpath,@loader_path/../Frameworks` for the
  bundled layout, plus a second rpath pointing at the vendored framework so a
  bare `cargo run` outside a bundle still resolves it.

Obtaining the framework can follow the `libghostty-vt-sys` precedent: a pinned
version constant, a checksum-verified download into a cache directory, and a
`prebuilt/` fallback for offline builds.

## 3. The bundling prerequisite

Nothing about Sparkle is reachable until there is a `scripts/bundle-mac.sh`
producing:

```
NiumaTerm.app/Contents/
  Info.plist
  MacOS/NiumaTerm
  MacOS/libtree_sitter.dylib        loaded by dlopen at runtime — see §6
  Resources/{AppIcon.icns, ...}
  Frameworks/Sparkle.framework      XPCServices removed
```

This is needed for the port regardless of the updater — a GPUI app without a
bundle has no icon, no dock name, and no `NSApplication` bundle identity — but
Sparkle makes it a hard blocker rather than a polish item. `Info.plist`
generation should be templated from `nmt_version` output so the version keys
cannot drift from the binary's stamped label.

## 4. Version scheme

Two constraints from Sparkle 2.9.6's headers decide this, and both cut against
the obvious design of "put the NiumaTerm label in `CFBundleVersion`":

- `SPUUpdaterDelegate.versionComparatorForUpdater:` is **deprecated**:
  *"Custom version comparators are deprecated because they are incompatible
  with how the system compares different versions of an app."*
- Even where a custom comparator is still honoured, the header warns that
  *"the standard version comparator may be used during installation for
  preventing a downgrade, even if you provide a custom comparator here."*

So `CFBundleVersion` has to be orderable by `SUStandardVersionComparator`
whatever else is done, and that comparator only understands numbers split by
character type. `nightly-20260821-7567b41` is not such a string.

**Design:**

| Field | Value | Why |
| --- | --- | --- |
| `CFBundleVersion` | HEAD committer Unix timestamp, e.g. `1788901234` | Strictly monotonic in time across every branch and both channels, collision-free without extra state, and a plain integer the standard comparator orders correctly |
| `CFBundleShortVersionString` | the `nmt_version` label verbatim | What Sparkle shows in its UI, and what the About page already displays |
| `sparkle:version` | same as `CFBundleVersion` | What Sparkle compares |
| `sparkle:shortVersionString` | same as `CFBundleShortVersionString` | What Sparkle displays |
| `<sparkle:channel>nightly</sparkle:channel>` | on nightly items only | Stable stays on the default channel |

`SPUUpdaterDelegate.allowedChannelsForUpdater:` returns `{"nightly"}` or `{}`
from `UpdateConfig::channel`, and `SPUUpdater.resetUpdateCycle` runs when the
setting changes. No custom comparator.

**On the default-channel leak.** Sparkle documents that *"an updater cannot
exclude itself from the default channel"*, so a user on nightly also sees
stable items. With a time-monotonic `CFBundleVersion` that is the correct
behaviour rather than a defect: a stable release is only offered to a nightly
user when it was actually cut after the nightly they are running, and the next
nightly (higher timestamp) moves them forward again. The alternative — two
feeds selected by `feedURLStringForUpdater:` — buys strict isolation at the
cost of a second published artifact, and Sparkle's own docs push away from it.

`nmt_version::Version` stays the authority for what the app *displays* and for
the Windows updater. On macOS it no longer decides ordering; the timestamp
does. That is a deliberate narrowing, not an oversight.

## 5. What an appcast is, and where to host it

An appcast is a static RSS 2.0 document with a Sparkle namespace. One `<item>`
per published build. Sparkle GETs it, discards items whose channel is not
allowed and whose OS/hardware requirements the machine fails, picks the highest
`sparkle:version` remaining, and if it beats the host's `CFBundleVersion`
offers it. On accept it downloads `<enclosure url>`, checks that the bytes
match `sparkle:edSignature` and `length`, and only then extracts and installs.
There is no server logic — the signature is what makes the file trustworthy,
not the transport.

For NiumaTerm it would look like:

```xml
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>NiumaTerm</title>
    <item>
      <title>nightly-20260906-9ff7517</title>
      <pubDate>Sun, 06 Sep 2026 11:04:00 +0000</pubDate>
      <sparkle:channel>nightly</sparkle:channel>
      <sparkle:version>1788901234</sparkle:version>
      <sparkle:shortVersionString>nightly-20260906-9ff7517</sparkle:shortVersionString>
      <sparkle:minimumSystemVersion>11.0</sparkle:minimumSystemVersion>
      <link>https://github.com/f32y/NiumaTerm/releases/tag/nightly-20260906</link>
      <enclosure
        url="https://github.com/f32y/NiumaTerm/releases/download/nightly-20260906/NiumaTerm-macos-universal.dmg"
        type="application/octet-stream"
        length="48213377"
        sparkle:edSignature="B4s2…=="/>
    </item>
    <item>
      <title>v1.2.9</title>
      <sparkle:version>1788470000</sparkle:version>
      <sparkle:shortVersionString>1.2.9</sparkle:shortVersionString>
      <!-- no sparkle:channel — the default channel, visible to everyone -->
      …
    </item>
  </channel>
</rss>
```

The archives stay on GitHub Releases; only the XML needs a home.

| Option | Cost | Notes |
| --- | --- | --- |
| **GitHub Pages on a `gh-pages` branch** | CI rewrites one file per release | Static, no rate limits, no runtime dependency, no token. A nightly commit per day lands on a branch nobody reads, so `main` stays clean and the pre-commit hooks never see it. |
| Fixed release tag | `gh release upload --clobber appcast appcast.xml` | Stable URL, no new hosting, but an opaque place for a document people will want to read |
| Cloudflare Worker | The repo already deploys `niumaterm-relay` via `wrangler.toml` | Renders the appcast from the Releases API, so GitHub Releases stays the single source of truth. Needs a GitHub token in a Worker secret (unauthenticated `api.github.com` is 60 req/h per IP, and a Worker's egress IPs are shared) plus Cache API use, and puts a live service in the update path. |

**Recommendation: GitHub Pages.** The Worker is the more elegant model and
becomes worth it if release notes rendering or phased rollout ever move
server-side, but for "publish one XML file" it adds a token, a cache policy and
an outage mode for no gain.

Building the file: `bin/generate_appcast` wants a directory holding every
archive so it can also emit delta updates, which means downloading the last N
releases in CI. For a first version it is simpler to render one `<item>` from
values CI already has (label, timestamp, URL, `wc -c`, `sign_update` output)
and prepend it to the fetched `appcast.xml`, trimming to the last ~10 items per
channel. Delta updates are a later optimisation and need `BinaryDelta` plus
retained old archives.

## 6. Certificates, signing, and notarization in CI

### Which certificate

Only **Developer ID Application** is needed. It signs the `.app`, the embedded
dylib, and the framework's Mach-Os, and it is what notarization checks.
*Developer ID Installer* is for `.pkg` distribution and does not apply to a
`.dmg` or `.zip`.

Create it from Xcode — Settings → Accounts → Manage Certificates → **+** →
Developer ID Application — which generates the keypair and CSR locally in one
step. The developer.apple.com portal route works too but needs a CSR from
Keychain Access first. On an organization account only the Account Holder can
create Developer ID certificates.

### Exporting to a `.p12`

In Keychain Access, switch the category to **My Certificates** — that view only
lists identities where the private key is present. If
`Developer ID Application: … (TEAMID)` is missing there, the private key lives
on whichever machine generated the CSR and the certificate alone is useless.

Select the identity → Export → *Personal Information Exchange (.p12)* → set an
export password. Then:

```sh
base64 -i DeveloperID.p12 | pbcopy
```

A trap worth knowing: Keychain writes `.p12` files with legacy RC2 encryption,
which OpenSSL 3 rejects by default. `security import` on the runner uses
Security.framework and is unaffected, but any step that pipes the file through
`openssl pkcs12` needs `-legacy`.

### Notarization credentials

`notarytool` needs an App Store Connect API key. It must be a **Team** key with
the **Developer** role — individual keys cannot reach the Notary API. App Store
Connect → Users and Access → Integrations → Keys → generate; the `.p8`
downloads exactly once. Record the Key ID (10 chars) and the Issuer ID (a
UUID).

The `--apple-id` / `--team-id` / `--password` form with an app-specific password
also works and is one secret fewer, but the key is revocable independently of
the Apple ID and does not break when 2FA state changes.

### GitHub secrets

| Secret | Content |
| --- | --- |
| `NMT_MACOS_CERT_P12` | base64 of the `.p12` |
| `NMT_MACOS_CERT_PASSWORD` | the `.p12` export password |
| `NMT_MACOS_SIGN_IDENTITY` | `Developer ID Application: Name (TEAMID)` |
| `NMT_MACOS_KEYCHAIN_PASSWORD` | any random string; scopes the throwaway keychain |
| `NMT_AC_API_KEY_P8` | base64 of `AuthKey_XXXXXXXXXX.p8` |
| `NMT_AC_API_KEY_ID` | the 10-character Key ID |
| `NMT_AC_API_ISSUER_ID` | the issuer UUID |
| `NMT_SPARKLE_ED_PRIVATE_KEY` | output of `bin/generate_keys -x` |

### Importing on the runner

```sh
KEYCHAIN="$RUNNER_TEMP/build.keychain-db"
security create-keychain -p "$NMT_MACOS_KEYCHAIN_PASSWORD" "$KEYCHAIN"
# The default 300 s auto-lock will fire mid-notarization on a long job.
security set-keychain-settings -lut 21600 "$KEYCHAIN"
security unlock-keychain -p "$NMT_MACOS_KEYCHAIN_PASSWORD" "$KEYCHAIN"
security list-keychains -d user -s "$KEYCHAIN" login.keychain-db

echo "$NMT_MACOS_CERT_P12" | base64 --decode > cert.p12
security import cert.p12 -k "$KEYCHAIN" -P "$NMT_MACOS_CERT_PASSWORD" \
  -T /usr/bin/codesign -T /usr/bin/security
rm cert.p12
# Without this, codesign blocks on a GUI prompt that never appears on a runner.
security set-key-partition-list -S apple-tool:,apple:,codesign: \
  -s -k "$NMT_MACOS_KEYCHAIN_PASSWORD" "$KEYCHAIN"

security find-identity -v -p codesigning "$KEYCHAIN"
```

### Signing order

Inside-out, and **never `--deep`** — it rewrites nested signatures that were
already correct and loses their entitlements. `--options runtime` (hardened
runtime) and `--timestamp` are both required for notarization.

```sh
SIGN=(codesign --force --options runtime --timestamp --sign "$NMT_MACOS_SIGN_IDENTITY")
APP=NiumaTerm.app
FW="$APP/Contents/Frameworks/Sparkle.framework"

"${SIGN[@]}" "$APP/Contents/MacOS/libtree_sitter.dylib"
"${SIGN[@]}" "$FW/Versions/B/Autoupdate"
"${SIGN[@]}" "$FW/Versions/B/Updater.app"
"${SIGN[@]}" "$FW"
"${SIGN[@]}" --entitlements assets/macos/NiumaTerm.entitlements "$APP"

codesign --verify --deep --strict --verbose=2 "$APP"
```

`libtree_sitter.dylib` is easy to forget and specific to this repo:
`crates/app/src/syntax/mod.rs` loads the tree-sitter bundle at runtime
(`LoadLibraryW` today, `dlopen` on macOS) from beside the executable. Under the
hardened runtime, library validation requires a `dlopen`ed Mach-O to carry the
same Team ID as the host. Signing it in CI satisfies that; the alternative —
a `com.apple.security.cs.disable-library-validation` entitlement — would also
disable the check that keeps a tampered Sparkle framework from loading, so it
is the wrong trade.

If the XPC services were kept, `Downloader.xpc` and `Installer.xpc` would sign
first, with `--preserve-metadata=entitlements` on the downloader. Deleting them
(§2) removes that step.

### Notarizing

Apple's prerequisites for the notary service, all of which the §6 signing step
already satisfies except the last two, which are worth checking once:

- every executable is code-signed, with a **Developer ID** certificate — not
  ad-hoc, not Apple Development, not Mac Distribution;
- the hardened runtime is enabled (`--options runtime`);
- the signature carries a secure timestamp (`--timestamp`);
- `com.apple.security.get-task-allow` is **not** present, in any form that
  evaluates to true. Xcode injects it into debug builds; a Cargo build never
  has it, but the entitlements plist must not add it;
- linked against the macOS 10.9 SDK or later;
- entitlements are well-formed, ASCII-encoded XML.

`altool` has been dead since 1 November 2023 — the notary service rejects its
uploads. `notarytool` (Xcode 13+) is the only CLI path, or the Notary REST API
if a machine without Xcode has to do it.

You cannot submit a `.app` directly. The notary service takes a **UDIF disk
image, a signed flat installer package, or a ZIP archive**, and it issues
tickets for nested items too — submitting a DMG that contains the app yields
tickets for both.

In CI, authenticate with the App Store Connect API key:

```sh
echo "$NMT_AC_API_KEY_P8" | base64 --decode > api_key.p8
ditto -c -k --sequesterRsrc --keepParent "$APP" notarize.zip

xcrun notarytool submit notarize.zip \
  --key api_key.p8 --key-id "$NMT_AC_API_KEY_ID" --issuer "$NMT_AC_API_ISSUER_ID" \
  --wait --timeout 30m --no-progress --output-format json
rm api_key.p8
```

`--issuer` is required for a Team key and must be omitted for an Individual
key — but an Individual key cannot reach the notary service at all, so a Team
key with the Developer role is the only workable choice. `--no-progress` keeps
the spinner out of the log; `--output-format json` makes the result parseable.
Locally, `xcrun notarytool store-credentials <profile>` puts the credentials in
the keychain and every later command takes `--keychain-profile <profile>`
instead of the three key flags.

Always read the log, even on success — it carries warnings that do not fail the
submission:

```sh
xcrun notarytool log "$SUBMISSION_ID" --key … developer_log.json
```

### Stapling, and why the order matters

`stapler` supports *UDIF disk images, code-signed executable bundles, and
signed flat installer packages*. **A ZIP cannot be stapled.** Apple's rule is
to staple each item that went into the archive and then build a fresh archive
from the stapled items:

```sh
xcrun stapler staple "$APP"
xcrun stapler validate "$APP"
spctl -a -t exec -vvv "$APP"

ditto -c -k --sequesterRsrc --keepParent "$APP" NiumaTerm-macos-universal.zip
```

So the sequence is sign → zip → notarize → **staple the `.app`** → re-zip →
`sign_update`. The zip submitted for notarization is a throwaway; the zip
Sparkle downloads is built afterwards from the stapled bundle. Getting this
backwards produces an app that works online and fails on a machine that is
offline at first launch, which is the hardest version of this bug to notice.

`--sequesterRsrc --keepParent` is not stylistic: the framework's code signature
depends on its `Versions/Current` symlinks surviving the round trip.

If a DMG is published for the website as well, it is a second notarization —
build it from the stapled `.app`, submit the DMG, and staple the DMG itself.

```sh
echo "$NMT_SPARKLE_ED_PRIVATE_KEY" > ed.key
./bin/sign_update --ed-key-file ed.key NiumaTerm-macos-universal.zip
rm ed.key
```

### Network, from behind a proxy

Relevant because this machine's shell sets `http_proxy`/`https_proxy` to
`127.0.0.1:7890`, which already breaks Zig's package fetcher:

- `notarytool` uploads through Amazon S3 Transfer Acceleration
  (`notary-submissions-prod.s3-accelerate.amazonaws.com`) by default.
  `--no-s3-acceleration` switches it to
  `notary-submissions-prod.s3.us-west-2.amazonaws.com`, which is the one to try
  when uploads stall.
- `stapler` fetches the ticket over CloudKit and needs Apple's `17.x` and
  `2620:149::` ranges reachable on port 443 — it is a separate network path
  from the submission, so a submission can succeed and the staple still fail.

GitHub-hosted macOS runners have neither problem; this matters for reproducing
a release locally.

### Timing and limits

Most submissions finish in under 5 minutes and 98% within 15, so `--wait
--timeout 30m` in CI is generous rather than optimistic. Apple asks for **no
more than 75 notarizations per day**, which one nightly plus the occasional
release never approaches. Two of Apple's latency rules apply here: keep the
file count down, and keep non-executable files out of `Contents/MacOS/` — they
belong in `Contents/Resources/`, which is not a code-signed location.

### Two things that are permanent

- Once `SUPublicEDKey` ships in a release, it cannot be removed —
  `SUUpdateValidator` refuses an update that drops either the EdDSA key or the
  code signing identity. Generate the key somewhere the release process keeps
  access to, and back up the export.
- Do not revoke the Developer ID certificate. A timestamped, notarized
  signature keeps validating after the certificate expires; revocation
  invalidates it.

## 7. Rust binding surface

`gpui_macos` already depends on `objc2`, `objc2-app-kit` and
`objc2-foundation`, so no new binding technology is needed. The whole surface
is small enough to hand-write; the two crates that exist upstream
(`hankbao/sparkle-updater` and its `sparkle-sys`) are unpublished on crates.io
and expose only `Updater::new()` / `check_for_updates()`, with no channel,
delegate or settings control. Not worth the dependency.

What has to be called, all on the main thread:

```
SPUStandardUpdaterController
  -initWithStartingUpdater:updaterDelegate:userDriverDelegate:   retained for app lifetime
  -updater                                                       → SPUUpdater

SPUUpdater
  -checkForUpdates                    user-initiated check, shows UI
  -canCheckForUpdates                 gate the About-page button (KVO-compliant)
  -setAutomaticallyChecksForUpdates:  mirror of [update] check-updates
  -setUpdateCheckInterval:
  -resetUpdateCycle                   after the channel setting changes
```

Plus one delegate class, defined at runtime with `objc2::define_class!`,
implementing `SPUUpdaterDelegate`:

- `-allowedChannelsForUpdater:` → `{"nightly"}` or `{}` from `UpdateConfig`
- `-updaterWillRelaunchApplication:` → flush `LocalState` before the swap

`-bestValidUpdateInAppcast:forUpdater:` exists and would hand full selection
control to Rust, but Sparkle's own header steers callers to
`allowedChannelsForUpdater:` instead, and with the §4 version scheme there is
nothing left for it to decide.

Placement: a new `crates/platform/src/macos/sparkle/` module, declared at the
crate root beside the existing `#[cfg(windows)] pub mod windows`. The `unix`
module is the `cfg(not(windows))` shared backend and is the wrong home for
something this Apple-specific.

## 8. User interface

`SPUStandardUserDriver` draws Sparkle's own Cocoa windows — the "A new version
is available" sheet, the progress bar, the release notes web view. They will
not look like the rest of NiumaTerm, but they are correct, localized into ~30
languages already, and free.

The alternative is implementing `SPUUserDriver` in Rust and rendering the flow
in GPUI. The protocol is around a dozen methods, most taking completion blocks
that must be stored and invoked later, and it owns the whole state machine of a
check. That is a meaningful project on its own and should not gate the first
working updater.

**Recommendation:** ship the standard driver. The About page keeps its existing
controls, with the "Check for Updates" button calling `checkForUpdates` and the
rich `Status` enum staying `cfg(windows)`. Add a "Check for Updates…" item to
the application menu, which is where a macOS user looks first.

## 9. CI

A new `.github/workflows/macos-package.yml`, mirroring `windows-package.yml`:

1. Build `aarch64-apple-darwin` and `x86_64-apple-darwin`; `lipo -create` the
   binary and `libtree_sitter.dylib` into universal Mach-Os.
2. `scripts/bundle-mac.sh` assembles the `.app`, embeds `Sparkle.framework`,
   deletes its `XPCServices/`, and renders `Info.plist` from `nmt_version`
   output plus the HEAD committer timestamp.
3. Import the certificate (§6), sign inside-out, notarize, staple.
4. `ditto -c -k --sequesterRsrc --keepParent` and `sign_update`.
5. `gh release create` / `upload` the archive.
6. Render the appcast `<item>` and push it to `gh-pages`.

Steps 1–2 also serve the nightly workflow, which today only builds Windows.

## 10. Rough order of work

1. `scripts/bundle-mac.sh` + templated `Info.plist` — needed by the port anyway.
2. Vendor `Sparkle.framework` and get it linking and loading from the bundle.
3. `SPUStandardUpdaterController` with the standard user driver, feed URL
   hardcoded, no channels — prove an update installs end to end, unsigned and
   served from a local HTTP server.
4. Delegate: channels and the relaunch hook; settings mirrored from
   `UpdateConfig`.
5. The signing and notarization job, then appcast hosting.
6. About page and application menu.

Steps 1–3 are the ones that can fail in surprising ways. 4–6 are mechanical.
Step 5 is the one that cannot be tested without the real certificate, so it is
worth doing on a throwaway tag before a real release depends on it.
