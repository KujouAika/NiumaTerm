#!/bin/sh
# Build, sign, notarize and staple NiumaTerm.dmg on a developer's own Mac.
#
# Signing and notarization cannot be exercised any other way than by running
# them, so this walks the same steps as `.github/workflows/macos-package.yml`
# rather than approximating them. What differs is where the credentials come
# from: the Developer ID identity and the notary key are read from the login
# keychain instead of from repository secrets, and the update feed is left
# alone unless it is asked for, because an image built here is for looking at
# rather than for anyone to update from.
#
# What it does not do is produce the Sparkle archive and its EdDSA signature.
# Those belong to a published release, and the private key that makes them
# stays where it is.
#
# Usage:
#   scripts/release-macos-local.sh
#   STAMP_FEED=1 NMT_SPARKLE_PUBLIC_ED_KEY=… scripts/release-macos-local.sh
#
# Environment:
#   NMT_MACOS_SIGN_IDENTITY  signing identity; the keychain's only Developer ID
#                            Application identity when unset
#   NMT_NOTARY_PROFILE       notarytool keychain profile (default
#                            `niumaterm-notary`), made by `notarytool
#                            store-credentials`
#   NIUMATERM_VERSION        version label (default `v` plus the crate version)
#   STAMP_FEED               `1` writes the release update metadata into the
#                            bundle, which needs NMT_SPARKLE_PUBLIC_ED_KEY
#   NMT_SPARKLE_FEED_URL     feed to stamp when STAMP_FEED is on
set -eu

out=dist
notary_profile=${NMT_NOTARY_PROFILE:-niumaterm-notary}
stamp_feed=${STAMP_FEED:-0}
feed_url=${NMT_SPARKLE_FEED_URL:-https://niumaterm-updates.f32.io/appcast.xml}
# Must match `min_macos` in scripts/bundle-macos.sh, which is what
# LSMinimumSystemVersion ends up saying: a binary built for a newer system than
# the bundle advertises fails at launch, not at build.
MACOSX_DEPLOYMENT_TARGET=13.0
export MACOSX_DEPLOYMENT_TARGET

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

# Ghostty's build refuses anything but the version its `build.zig.zon` names,
# and says so only once the Zig build is already running. A keg-only Homebrew
# install is not on PATH by itself, so it is looked for by name.
zig_version=0.16.0
if ! zig version 2>/dev/null | grep -qx "$zig_version"; then
  keg=/opt/homebrew/opt/zig@0.16/bin
  if [ -x "$keg/zig" ]; then
    PATH="$keg:$PATH"
    export PATH
  else
    echo "zig $zig_version is required and was not found" >&2
    echo "install it with: brew install zig@0.16" >&2
    exit 1
  fi
fi

# Zig 0.15's package fetcher cannot negotiate TLS through an HTTP proxy and
# fails with `TlsInitializationFailed` rather than falling back. Cleared for
# this script's own children only.
unset HTTP_PROXY http_proxy HTTPS_PROXY https_proxy

identity=${NMT_MACOS_SIGN_IDENTITY:-}
if [ -z "$identity" ]; then
  # The name, not the hash, so the failure below reads as something a person
  # can act on. Exactly one match is required: signing a release with whichever
  # identity happened to sort first is not a decision to make silently.
  identity=$(security find-identity -v -p codesigning |
    sed -n 's/.*"\(Developer ID Application: .*\)"$/\1/p')
  case $(printf '%s' "$identity" | grep -c .) in
    1) ;;
    0)
      echo "no Developer ID Application identity in the keychain" >&2
      exit 1
      ;;
    *)
      echo "several Developer ID Application identities; name one in" >&2
      echo "NMT_MACOS_SIGN_IDENTITY:" >&2
      printf '%s\n' "$identity" >&2
      exit 1
      ;;
  esac
fi

version=${NIUMATERM_VERSION:-v$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)}
export NIUMATERM_VERSION=$version
app="$out/NiumaTerm.app"
dmg="$out/NiumaTerm-macos-arm64-$version.dmg"

echo "==> building $version as $identity"
# `shell_extension` is an Explorer context-menu DLL and is in default-members,
# so a bare `cargo build` builds more than this package needs; name the two.
cargo build --release --locked -p app -p nmt_tree_sitter_bundle

echo "==> assembling the bundle"
./scripts/bundle-macos.sh --profile release --out "$out"

if [ "$stamp_feed" = 1 ]; then
  echo "==> stamping the update metadata"
  : "${NMT_SPARKLE_PUBLIC_ED_KEY:?the public key the released builds carry}"
  plist="$app/Contents/Info.plist"
  # Sparkle orders releases with SUStandardVersionComparator, which reads
  # numbers only. The committer timestamp is the one value that increases
  # monotonically across both publishing channels and every branch.
  plutil -replace CFBundleVersion -string "$(git show -s --format=%ct HEAD)" "$plist"
  plutil -replace SUFeedURL -string "$feed_url" "$plist"
  plutil -replace SUPublicEDKey -string "$NMT_SPARKLE_PUBLIC_ED_KEY" "$plist"
  # NiumaTerm asks about automatic checks in its own settings; without this key
  # Sparkle asks again with its own dialog on first launch.
  plutil -replace SUEnableAutomaticChecks -bool true "$plist"
  # Six hours, matching the Windows updater's CHECK_INTERVAL.
  plutil -replace SUScheduledCheckInterval -integer 21600 "$plist"
  plutil -lint "$plist"
fi

echo "==> signing"
# Inside-out, and never --deep: --deep rewrites nested signatures that are
# already correct and drops the entitlements they were signed with.
# --options runtime and --timestamp are both notarization requirements.
#
# No entitlements file: the hardened runtime with no exceptions is what this
# application needs. It spawns shells rather than loading plug-ins, and every
# Mach-O it opens is signed by the same team.
framework="$app/Contents/Frameworks/Sparkle.framework"
sign() { codesign --force --options runtime --timestamp --sign "$identity" "$@"; }

# crates/app/src/syntax opens this with dlopen at runtime. Under the hardened
# runtime, library validation accepts a dlopened Mach-O only when it carries
# the host's Team ID, so it is signed rather than excused with a
# disable-library-validation entitlement — which would also stop enforcing the
# same check on the framework.
sign "$app/Contents/MacOS/libtree_sitter.dylib"
sign "$framework/Versions/B/Autoupdate"
sign "$framework/Versions/B/Updater.app"
sign "$framework"
sign "$app"
codesign --verify --deep --strict --verbose=2 "$app"

echo "==> notarizing the bundle"
# The notary service does not accept a .app; it takes a UDIF image, a signed
# flat package, or a zip. This archive is a throwaway: the ticket is stapled to
# the bundle, and a zip cannot be stapled at all.
staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT
ditto -c -k --sequesterRsrc --keepParent "$app" "$staging/notarize.zip"
xcrun notarytool submit "$staging/notarize.zip" \
  --keychain-profile "$notary_profile" --wait --timeout 30m --no-progress
xcrun stapler staple "$app"
# Validating here rather than after download is what distinguishes a stapled
# bundle from one whose ticket is merely findable online; the difference only
# shows on a machine that is offline at first launch.
xcrun stapler validate "$app"
spctl -a -t exec -vvv "$app"

echo "==> building the disk image"
# The window layout comes from a recorded .DS_Store: writing one needs Finder,
# and only a desktop session can grant the automation this takes.
./scripts/make-dmg.sh "$app" "$dmg"

echo "==> notarizing the disk image"
# Its own submission rather than the app's, because whatever reaches a user has
# to carry the ticket rather than merely be findable online, and an image can
# be stapled where the zip the bundle went up in cannot.
codesign --force --timestamp --sign "$identity" "$dmg"
xcrun notarytool submit "$dmg" \
  --keychain-profile "$notary_profile" --wait --timeout 30m --no-progress
xcrun stapler staple "$dmg"
xcrun stapler validate "$dmg"
spctl -a -t open --context context:primary-signature -vvv "$dmg"

echo
echo "$dmg"
