#!/bin/sh
# Assemble NiumaTerm.app around an already-built binary.
#
# A bare executable is not an application as far as macOS is concerned: it has
# no bundle identifier, which is what `UNUserNotificationCenter` refuses to
# work without and what the permissions the user grants are remembered against.
# It also cannot carry an icon or become a regular, activatable application.
#
# Usage:
#   scripts/bundle-macos.sh [--profile release] [--binary PATH] [--out DIR]
#                           [--identifier ID] [--icon PNG] [--sign IDENTITY]
#
# The signing identity defaults to `-`, an ad-hoc signature, which is enough to
# run the result locally. Distribution needs a Developer ID identity and, after
# that, notarization.
set -eu

profile=release
binary=
out=dist
identifier=${NMT_BUNDLE_ID:-io.f32.NiumaTerm}
# The same 512px source the Windows icon was cut from; there is no larger one
# in the tree, so the 1024px `512x512@2x` slice is left out rather than faked
# by upscaling.
icon=assets/windows/app-512.png
sign=-
min_macos=13.0

while [ $# -gt 0 ]; do
  case $1 in
    --profile) profile=$2; shift 2 ;;
    --binary) binary=$2; shift 2 ;;
    --out) out=$2; shift 2 ;;
    --identifier) identifier=$2; shift 2 ;;
    --icon) icon=$2; shift 2 ;;
    --sign) sign=$2; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

[ -n "$binary" ] || binary="target/$profile/NiumaTerm"
if [ ! -x "$binary" ]; then
  echo "no executable at $binary; build it first" >&2
  exit 1
fi

# Apple silicon is the only supported target, so a binary without an arm64
# slice would produce a bundle that cannot run where it is meant to — and
# nothing later in the assembly would notice.
if ! lipo -archs "$binary" | tr ' ' '\n' | grep -qx arm64; then
  echo "$binary is $(lipo -archs "$binary"); an arm64 slice is required" >&2
  exit 1
fi

# `CFBundleShortVersionString` and `CFBundleVersion` must be dotted numbers, so
# they take the crate version. The build's own label — which is what the
# application compares against a release feed, and which for a nightly is not a
# dotted number at all — is carried beside them.
short_version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
version_label=${NIUMATERM_VERSION:-v$short_version}

app="$out/NiumaTerm.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

# `iconutil` reads a directory of exactly these names; anything else in it is
# an error rather than an ignored file.
iconset=$(mktemp -d)
trap 'rm -rf "$iconset"' EXIT
mkdir -p "$iconset/AppIcon.iconset"
for spec in 16:16x16 32:16x16@2x 32:32x32 64:32x32@2x 128:128x128 256:128x128@2x 256:256x256 512:256x256@2x 512:512x512; do
  pixels=${spec%%:*}
  name=${spec#*:}
  sips -z "$pixels" "$pixels" "$icon" --out "$iconset/AppIcon.iconset/icon_$name.png" >/dev/null
done
iconutil --convert icns --output "$app/Contents/Resources/AppIcon.icns" "$iconset/AppIcon.iconset"

cp "$binary" "$app/Contents/MacOS/NiumaTerm"

sed \
  -e "s|@@BUNDLE_ID@@|$identifier|g" \
  -e "s|@@SHORT_VERSION@@|$short_version|g" \
  -e "s|@@VERSION_LABEL@@|$version_label|g" \
  -e "s|@@MIN_MACOS@@|$min_macos|g" \
  assets/macos/Info.plist > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist" >/dev/null

# Classic-era metadata that a few tools still read to identify the bundle kind.
printf 'APPL????' > "$app/Contents/PkgInfo"

# Signed last: a signature covers the bundle's contents, so anything written
# afterwards invalidates it.
codesign --force --sign "$sign" --timestamp=none "$app" >/dev/null 2>&1 ||
  codesign --force --sign "$sign" "$app"
codesign --verify --strict "$app"

echo "$app"
