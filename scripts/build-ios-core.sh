#!/bin/sh
# Build the Rust core of the iOS app (crates/mobile) and package it for Xcode.
#
# Produces, under mobile/ios/Packages/NiumaTermCore:
#   NiumaTermCoreFFI.xcframework   the static library for devices and for the
#                                  Apple-silicon simulator, with its C header
#   Sources/NiumaTermCore/         the UniFFI-generated Swift bindings
#
# The Swift package there wraps both, so the app imports one module. Both are
# build outputs and are not committed; run this after changing Rust code the
# app links.
#
#   scripts/build-ios-core.sh            release build (what the app ships)
#   scripts/build-ios-core.sh --debug    debug build, faster to compile
#   scripts/build-ios-core.sh --sim      simulator only, skips the device slice
#
# Needs the aarch64-apple-ios and aarch64-apple-ios-sim Rust targets
# (`rustup target add aarch64-apple-ios aarch64-apple-ios-sim`) and Xcode.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
package="$root/mobile/ios/Packages/NiumaTermCore"

profile=release
profile_dir=release
targets="aarch64-apple-ios aarch64-apple-ios-sim"

for arg in "$@"; do
    case $arg in
        --debug) profile=dev; profile_dir=debug ;;
        --sim) targets="aarch64-apple-ios-sim" ;;
        *) echo "unknown option: $arg" >&2; exit 2 ;;
    esac
done

cd "$root"

# The deployment target the app declares; objects built for a newer one
# would make the linker warn on every build.
export IPHONEOS_DEPLOYMENT_TARGET=26.0

for target in $targets; do
    cargo build -p nmt_mobile --lib --profile "$profile" --target "$target"
done

first=$(echo "$targets" | cut -d' ' -f1)
library="$root/target/$first/$profile_dir/libnmt_mobile.a"

generated=$(mktemp -d)
trap 'rm -rf "$generated"' EXIT

cargo run -q -p uniffi-bindgen -- generate \
    --library "$library" \
    --language swift \
    --out-dir "$generated"

# An xcframework carries one header directory per slice; the module map has
# to be named module.modulemap for Swift to find the C module in it.
headers="$generated/headers"
mkdir -p "$headers"
cp "$generated/NiumaTermCoreFFI.h" "$headers/"
cp "$generated/NiumaTermCoreFFI.modulemap" "$headers/module.modulemap"

rm -rf "$package/NiumaTermCoreFFI.xcframework"

set --
for target in $targets; do
    set -- "$@" -library "$root/target/$target/$profile_dir/libnmt_mobile.a" -headers "$headers"
done

xcodebuild -create-xcframework "$@" -output "$package/NiumaTermCoreFFI.xcframework"

mkdir -p "$package/Sources/NiumaTermCore"
cp "$generated/NiumaTermCore.swift" "$package/Sources/NiumaTermCore/NiumaTermCore.swift"

echo "NiumaTermCore built ($profile) for: $targets"
