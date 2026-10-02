#!/bin/sh
# Build the Rust core of the iOS app (crates/mobile) for the platform and
# configuration Xcode is building, and generate its Swift bindings.
#
# The NiumaTermCore target in mobile/ios runs this as its first build phase,
# so building the app in Xcode or with xcodebuild keeps the core current.
# It produces:
#   $BUILT_PRODUCTS_DIR/libnmt_mobile.a       linked into the app
#   mobile/ios/NiumaTermCore/Generated/       the UniFFI Swift bindings and
#                                             the NiumaTermCoreFFI C module
#
# Needs the aarch64-apple-ios and aarch64-apple-ios-sim Rust targets
# (`rustup target add aarch64-apple-ios aarch64-apple-ios-sim`) and the Zig
# version the desktop build uses, both reachable from a login shell.
set -eu

: "${PLATFORM_NAME:?run this from the NiumaTermCore target in Xcode}"
: "${CONFIGURATION:?run this from the NiumaTermCore target in Xcode}"
: "${BUILT_PRODUCTS_DIR:?run this from the NiumaTermCore target in Xcode}"

root=$(cd "$(dirname "$0")/.." && pwd)
generated="$root/mobile/ios/NiumaTermCore/Generated"

# Only the Apple-silicon simulator is supported; the project excludes x86_64
# from simulator builds so ARCHS never asks for a second slice.
case $PLATFORM_NAME in
    iphoneos) target=aarch64-apple-ios ;;
    iphonesimulator) target=aarch64-apple-ios-sim ;;
    *) echo "error: unsupported platform $PLATFORM_NAME" >&2; exit 1 ;;
esac

case $CONFIGURATION in
    Debug) profile=dev; profile_dir=debug ;;
    *) profile=release; profile_dir=release ;;
esac

# Xcode exports its build settings to this script, and some of them break
# cargo: SDKROOT points at the iOS SDK, so build scripts and proc macros,
# which are linked for the Mac, fail to link against it. Xcode started from
# the Dock also has launchd's bare PATH, without cargo or Zig. Run cargo in
# a clean login shell, which gets the same PATH as a terminal, and pass on
# only the settings the iOS build needs.
in_login_shell() {
    env -i HOME="$HOME" USER="${USER:-}" LOGNAME="${LOGNAME:-}" \
        TMPDIR="${TMPDIR:-/tmp}" \
        DEVELOPER_DIR="${DEVELOPER_DIR:-}" \
        IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-26.0}" \
        "${SHELL:-/bin/zsh}" -lc 'cd "$0" && exec "$@"' "$root" "$@"
}

in_login_shell cargo build -p nmt_mobile --lib --profile "$profile" --target "$target"

library="$root/target/$target/$profile_dir/libnmt_mobile.a"

# Copy only a changed library: its timestamp decides whether Xcode relinks
# the app.
mkdir -p "$BUILT_PRODUCTS_DIR"
if ! cmp -s "$library" "$BUILT_PRODUCTS_DIR/libnmt_mobile.a"; then
    cp "$library" "$BUILT_PRODUCTS_DIR/libnmt_mobile.a"
fi

# The bindings depend only on the crate's exported interface, which is the
# same for every target and profile. Regenerate them only when the library
# is newer than the last run, and replace each file only when it changed,
# so an unchanged core does not recompile the Swift that imports it.
stamp="$generated/.stamp"
if [ -f "$stamp" ] && [ "$stamp" -nt "$library" ]; then
    exit 0
fi

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

in_login_shell cargo run -q -p uniffi-bindgen -- generate \
    --library "$library" \
    --language swift \
    --out-dir "$scratch"

# Swift finds a C module through a file named module.modulemap in a
# directory on SWIFT_INCLUDE_PATHS.
mv "$scratch/NiumaTermCoreFFI.modulemap" "$scratch/module.modulemap"

mkdir -p "$generated"
for file in NiumaTermCore.swift NiumaTermCoreFFI.h module.modulemap; do
    if ! cmp -s "$scratch/$file" "$generated/$file"; then
        cp "$scratch/$file" "$generated/$file"
    fi
done
touch "$stamp"
