# macOS Release CI Configuration

| Field | Value |
| --- | --- |
| Status | Called from `release.yml` and `nightly.yml`; arm64 only |
| Date | 2026-09-06 |
| Scope | Building, signing, notarizing and publishing `NiumaTerm.app`, and keeping the Sparkle appcast current |
| Companion | [`macos-sparkle-auto-update.md`](./macos-sparkle-auto-update.md) — why each step is shaped this way |

## 1. What is here

| Path | Role |
| --- | --- |
| `.github/workflows/macos-package.yml` | Build → bundle → embed Sparkle → sign → notarize → archive. Reusable, plus `workflow_dispatch` so the signing path can be rehearsed |
| `.github/workflows/appcast.yml` | Publishes one release into the feed asset |
| `scripts/update-appcast.py` | Renders and trims the appcast document |

built on top of two files the macOS port already owns:

| Path | Role |
| --- | --- |
| `scripts/bundle-macos.sh` | Assembles `NiumaTerm.app`: icon, binary, `libtree_sitter.dylib`, `Info.plist`, ad-hoc signature |
| `assets/macos/Info.plist` | Bundle metadata template with `@@…@@` placeholders |

`release.yml` and `nightly.yml` are deliberately unchanged — see §6.

## 2. One-time setup

Six things CI cannot do for itself, in dependency order.

### 2.1 Developer ID Application certificate

Xcode → Settings → Accounts → Manage Certificates → **+** → *Developer ID
Application*. On an organization account only the Account Holder may create one.

Export it: Keychain Access → category **My Certificates** → select
`Developer ID Application: … (TEAMID)` → Export → *Personal Information
Exchange (.p12)* → set a password. If the identity is not under *My
Certificates*, the private key is on the machine that made the CSR and the
certificate alone is useless.

```sh
base64 -i DeveloperID.p12 | pbcopy          # → NMT_MACOS_CERT_P12
security find-identity -v -p codesigning    # → NMT_MACOS_SIGN_IDENTITY, copied verbatim
```

Keychain writes `.p12` files with legacy RC2 encryption, which OpenSSL 3
rejects. `security import` on the runner uses Security.framework and is
unaffected, but any step that pipes the file through `openssl pkcs12` needs
`-legacy`.

### 2.2 App Store Connect API key

App Store Connect → Users and Access → Integrations → Keys → generate a **Team**
key with the **Developer** role. Individual keys cannot reach the notary
service. The `.p8` downloads exactly once.

```sh
base64 -i AuthKey_XXXXXXXXXX.p8 | pbcopy    # → NMT_AC_API_KEY_P8
```

Record the Key ID (10 characters) and the Issuer ID (a UUID).

### 2.3 Sparkle EdDSA key

```sh
curl -fsSLO https://github.com/sparkle-project/Sparkle/releases/download/2.9.6/Sparkle-2.9.6.tar.xz
echo '52bf9e88cdd972fc0c81501377a880e90d47031bd8ca5462488f843e2609e192  Sparkle-2.9.6.tar.xz' \
  | shasum -a 256 -c
mkdir sparkle && tar -xJf Sparkle-2.9.6.tar.xz -C sparkle

./sparkle/bin/generate_keys                 # private key → login keychain; prints the public key
./sparkle/bin/generate_keys -x ed.key       # export the private half
```

The public key is not a secret; it is stamped into every bundle. Store it as a
repository **variable** named `NMT_SPARKLE_PUBLIC_ED_KEY` — the packaging job fails
early and says so if it is unset, because a build that shipped without it can
never be updated.

Store `ed.key`'s contents as the secret `NMT_SPARKLE_ED_PRIVATE_KEY`, back the file
up somewhere durable, and delete it from the working directory. Sparkle refuses
an update that drops the public key an installed copy carries, so losing the
private half ends the update path for everyone already on macOS.

### 2.4 Where the feed lives

Nothing to set up. The feed is the single asset of a prerelease under the fixed
tag `appcast`, and `appcast.yml` creates that release the first time it runs.
It lands at

```text
https://github.com/f32y/NiumaTerm/releases/download/appcast/appcast.xml
```

which is the URL `macos-package.yml` stamps as `SUFeedURL`, derived there from
`github.repository` so a fork points at its own feed. The release is marked a
prerelease so it never becomes the repository's "Latest release": it holds a
document, not a build anyone downloads.

This address cannot change once a build carrying it has shipped. An
installation only ever asks the URL it was built with, so moving the feed
strands every copy already out there.

### 2.5 Repository secrets and variables

Settings → Secrets and variables → Actions.

| Secret | Value |
| --- | --- |
| `NMT_MACOS_CERT_P12` | base64 of the `.p12` |
| `NMT_MACOS_CERT_PASSWORD` | the `.p12` export password |
| `NMT_MACOS_SIGN_IDENTITY` | `Developer ID Application: Name (TEAMID)` |
| `NMT_MACOS_KEYCHAIN_PASSWORD` | any random string; scopes the throwaway keychain |
| `NMT_AC_API_KEY_P8` | base64 of `AuthKey_XXXXXXXXXX.p8` |
| `NMT_AC_API_KEY_ID` | the 10-character Key ID |
| `NMT_AC_API_ISSUER_ID` | the issuer UUID |
| `NMT_SPARKLE_ED_PRIVATE_KEY` | contents of `ed.key` |

| Variable | Value |
| --- | --- |
| `NMT_SPARKLE_PUBLIC_ED_KEY` | the public key `generate_keys` printed |

### 2.6 Bundle identifier

`io.f32.NiumaTerm`, already the default in `scripts/bundle-macos.sh`. It is
permanent in three separate ways: Sparkle's preferences live under it in
`NSUserDefaults`, macOS keys TCC grants to it, and changing it makes every
installed copy look like a different application.

## 3. How the packaging job is put together

Apple silicon is the only supported target. `macos-package.yml` builds
`aarch64-apple-darwin` alone, `bundle-macos.sh` rejects a binary with no arm64
slice, and the published artifacts say `arm64` in their names, so an Intel Mac
is told what it is looking at rather than handed something that will not run.

Shipping a second slice is not planned. `libghostty-vt-sys` can target
`x86_64-macos-none` through Zig, so the door is not nailed shut, but every
artifact, the bundle script's check and the artifact names would have to change
together.

Two details of this repository shape the build step:

- `shell_extension` is an Explorer context-menu DLL and sits in
  `default-members`, so a bare `cargo build` fails on macOS. The job names
  `-p app -p nmt_tree_sitter_bundle`.
- `MACOSX_DEPLOYMENT_TARGET` is set to `13.0` to match `min_macos` in
  `bundle-macos.sh`. A binary built for a newer system than
  `LSMinimumSystemVersion` advertises fails at launch rather than at build.
  Sparkle 2.9.6 itself is built for 11.0, so it imposes no further floor.

### What the job adds on top of `bundle-macos.sh`

`bundle-macos.sh` produces an application anyone can run locally. Three things
belong to a *published* build rather than to any bundle, so the job adds them
afterwards:

1. **`Sparkle.framework`** into `Contents/Frameworks`, copied with `ditto`
   because the framework's signature seals its `Versions/Current` symlinks, and
   with `Versions/B/XPCServices` deleted — those exist so a sandboxed host can
   reach the network and the installer out of process, and a terminal emulator
   cannot be sandboxed.
2. **Distribution metadata**, stamped with `plutil`: `SUFeedURL`,
   `SUPublicEDKey`, `SUEnableAutomaticChecks`, `SUScheduledCheckInterval`, and
   `CFBundleVersion`. A locally assembled app has no business pointing at the
   live feed, which is why these are not in `assets/macos/Info.plist`.
3. **A real signature**, replacing the script's ad-hoc one.

`CFBundleVersion` is overwritten with the HEAD committer timestamp.
`bundle-macos.sh` sets it to the crate version, which is right for
LaunchServices but cannot order two nightlies against each other — every
nightly would report `1.2.9`. Sparkle compares with
`SUStandardVersionComparator`, which reads numbers only; the committer
timestamp is the one value that increases monotonically across both channels
and every branch. `CFBundleShortVersionString` keeps the crate version and
`NMTVersionLabel` keeps the full label, so nothing a user reads changes.

### Signing order

Inside out, and never `--deep` — that rewrites nested signatures which are
already correct and drops the entitlements they were signed with.
`--options runtime` and `--timestamp` are both notarization requirements.

```
libtree_sitter.dylib
Sparkle.framework/Versions/B/Autoupdate
Sparkle.framework/Versions/B/Updater.app
Sparkle.framework
NiumaTerm.app
```

`libtree_sitter.dylib` is the one that gets forgotten: `crates/app/src/syntax`
opens it with `dlopen` at runtime, and under the hardened runtime library
validation only accepts a `dlopen`ed Mach-O carrying the host's Team ID. The
alternative, a `com.apple.security.cs.disable-library-validation` entitlement,
would also stop enforcing that check on the Sparkle framework, so it is the
wrong trade.

No entitlements file is passed at all. The hardened runtime with no exceptions
is what this app needs — it spawns shells rather than loading plug-ins.
`com.apple.security.get-task-allow` must never be added; the notary service
rejects it.

### Notarizing and stapling

The notary service does not accept a `.app`. It takes a UDIF disk image, a
signed flat installer package, or a ZIP, and **a ZIP cannot be stapled**. So the
archive submitted for notarization is a throwaway, the ticket is stapled to the
bundle, and the distributed archive is built from the stapled bundle
afterwards:

```
sign → ditto → notarytool submit --wait → stapler staple the .app → ditto again → sign_update
```

Getting that backwards produces an app that works online and fails on a machine
that is offline at first launch, which is the hardest version of this bug to
notice. `stapler validate` runs inside the job for the same reason: passing only
on a later manual run means the ticket was fetched online and the bundle is not
actually stapled.

`--sequesterRsrc --keepParent` on both `ditto` calls is not stylistic — the
framework's signature depends on its symlinks surviving the round trip.

`sign_update -p` prints only the signature, and the length comes from `wc -c`,
so nothing has to parse the tool's attribute fragment. The key is passed with
`--ed-key-file` rather than `-s`, because an argument is readable from the
process list.

## 4. The appcast

`scripts/update-appcast.py` prepends one `<item>`, sorts by
`<sparkle:version>`, and keeps the newest 10 per channel. Nightly items carry
`<sparkle:channel>nightly</sparkle:channel>`; stable items carry no channel
element, which is what puts them on the default channel every updater can see.

Republishing a revision replaces its existing item rather than adding a second
one, so rerunning the nightly job on a revision it already published is safe.

`generate_appcast` would do this too, and would generate delta updates, but it
needs every previous archive present in one directory — which means downloading
the last N releases on every run. The values it would compute are ones the
packaging job already measured.

## 5. Publishing order

The appcast job must run **after** the GitHub release exists. Sparkle fetches
the enclosure URL directly, so an item published against an asset that is not
up yet is a 404 for every client that checks in between.

## 6. The remaining step: calling these from the release workflows

Not applied, because `crates/app` does not build on macOS yet and a red macOS
job on every release teaches people to ignore the check. When the port lands,
`release.yml` becomes:

```yaml
jobs:
  package:
    uses: ./.github/workflows/windows-package.yml
    with:
      ref: ${{ inputs.tag || github.ref_name }}
      version: ${{ inputs.tag || github.ref_name }}

  package-macos:
    uses: ./.github/workflows/macos-package.yml
    with:
      ref: ${{ inputs.tag || github.ref_name }}
      version: ${{ inputs.tag || github.ref_name }}
    secrets: inherit

  release:
    needs: [package, package-macos]
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - uses: actions/download-artifact@v4
        with:
          # Both packaging jobs name their artifact after the version, so one
          # pattern collects them without repeating either name.
          pattern: NiumaTerm-*-${{ inputs.tag || github.ref_name }}
          merge-multiple: true
          path: dist
      - env:
          GH_TOKEN: ${{ github.token }}
          TAG: ${{ inputs.tag || github.ref_name }}
        run: >-
          gh release create "$TAG" dist/*
          --repo "$GITHUB_REPOSITORY"
          --title "$TAG"
          --generate-notes

  appcast:
    needs: [package-macos, release]
    uses: ./.github/workflows/appcast.yml
    with:
      label: ${{ inputs.tag || github.ref_name }}
      bundle-version: ${{ needs.package-macos.outputs.bundle-version }}
      channel: ''
      asset: ${{ needs.package-macos.outputs.asset }}
      length: ${{ needs.package-macos.outputs.length }}
      signature: ${{ needs.package-macos.outputs.ed-signature }}
```

`nightly.yml` takes the same two jobs with `ref: ${{ github.sha }}`,
`version: ${{ needs.name.outputs.tag }}` and `channel: 'nightly'`, and its
existing download step widened to the same pattern.

## 7. First run

Do this on a throwaway tag before a real release depends on it. Signing and
notarization cannot be exercised any other way, and `macos-package.yml` accepts
a manual dispatch for exactly this.

Check, in order:

1. **Signing** — `codesign --verify --deep --strict` and `spctl -a -t exec -vvv`
   both pass in the job, and `spctl` reports `source=Notarized Developer ID`.
2. **Stapling** — `stapler validate` passes *in the job*.
3. **The archive** — download the published zip on a different Mac, unpack,
   launch. Nothing beyond the ordinary first-run dialog.
4. **The feed** — `curl -L` against
   `https://github.com/f32y/NiumaTerm/releases/download/appcast/appcast.xml`
   returns the document, and its enclosure URL downloads.
5. **The update** — install the test build, publish a second tag, let the app
   find it. This is the only step that exercises Sparkle end to end, and the
   only one that catches a `CFBundleVersion` that failed to increase.

Then delete the tag, the releases, and the appcast items.

## 8. Where this fails, and what it looks like

| Symptom | Cause |
| --- | --- |
| `codesign` hangs with no output | `set-key-partition-list` was skipped, or the keychain auto-locked after its default 300 s |
| Notarization returns `Invalid` | Read `notarytool log <id>`; almost always one unsigned nested Mach-O, and `libtree_sitter.dylib` is the usual one |
| App launches from the runner but not elsewhere | The zip was stapled instead of the bundle, or rebuilt before stapling |
| Sparkle finds nothing | `CFBundleVersion` did not increase, or the item's channel is not in `allowedChannelsForUpdater:` |
| Sparkle finds it and refuses to install | The signature was computed over a different file than the one published — usually the pre-staple archive |
| Framework fails to load at launch | The archiver resolved `Versions/Current`; use `ditto`, not `zip` |
| Upload stalls in `notarytool` | S3 Transfer Acceleration is unreachable; add `--no-s3-acceleration` |

## 9. Not covered

- **`NmtAgentHook`.** The Windows package ships it; `bundle-macos.sh` does not
  copy it and the job does not build it. Whatever the macOS agent-hook story
  turns out to be, it belongs in the bundle script rather than here.
- **A DMG for the website.** Sparkle is content with the zip. A DMG is a second
  notarization of its own: build it from the stapled `.app`, submit the image,
  staple the image.
- **Delta updates**, which need `BinaryDelta` and retained old archives.
- **A `niumaterm` CLI on `PATH`.** Windows gets it from the installer.
- **Folding the Sparkle keys into `assets/macos/Info.plist`.** They are stamped
  by the job today. If a locally bundled app ever needs to check for updates,
  the feed URL and public key move into the template and gain their own
  `bundle-macos.sh` flags.
