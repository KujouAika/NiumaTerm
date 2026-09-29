#!/bin/sh
# Rebuild the checked-in macOS libghostty-vt prebuilt package from source.
#
# `libghostty-vt-sys` either builds libghostty-vt from the pinned Ghostty
# sources with Zig (the default) or, with NMT_USE_PREBUILT_LIBGHOSTTY=1, links
# the package under third_party/libghostty-vt-sys/prebuilt/<target>/. A
# prebuilt build needs neither Zig nor a Ghostty checkout, and an unoptimized
# build still gets an optimized parser, where the vendored Debug build parses
# about 2000 times slower. Regenerate the package whenever the pinned Ghostty
# commit or the vendored source patches change.
#
# The package is lib/libghostty-vt.a plus the include/ headers. On macOS Zig
# folds simdutf and highway into that one archive, so unlike the Windows
# package there are no separate dependency archives to ship.
#
# Usage:
#   scripts/update-libghostty-prebuilt-macos.sh [--target <triple>]
#       [--optimize <mode>] [--skip-verify]
#
#   --target    Rust target triple; default aarch64-apple-darwin, the only
#               macOS architecture the app ships for
#   --optimize  Zig optimize mode: Debug, ReleaseSafe, ReleaseFast (default),
#               or ReleaseSmall
#   --skip-verify
#               skip linking nmt_terminal's tests against the new package
#
# Environment:
#   CARGO_TARGET_DIR          parent of the scratch target directory; the build
#                             runs in <dir>/libghostty-prebuilt so the regular
#                             build keeps its artifacts and optimize mode
#   MACOSX_DEPLOYMENT_TARGET  defaults to 13.0, the release app's minimum; the
#                             archive must not require a newer macOS
#
# Requires zig (the version the pinned Ghostty's build.zig.zon names), cargo,
# and the rustup target. The first run clones Ghostty into the scratch target
# directory, so git must be able to reach GitHub.
set -eu

target=aarch64-apple-darwin
optimize=ReleaseFast
verify=1

usage() {
  sed -n '16,26p' "$0" | sed 's/^# \{0,1\}//'
}

die() {
  echo "error: $*" >&2
  exit 1
}

while [ $# -gt 0 ]; do
  case "$1" in
    --target)
      [ $# -ge 2 ] || die "--target needs a value"
      target=$2
      shift 2
      ;;
    --optimize)
      [ $# -ge 2 ] || die "--optimize needs a value"
      optimize=$2
      shift 2
      ;;
    --skip-verify)
      verify=0
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      usage >&2
      exit 2
      ;;
  esac
done

case "$target" in
  aarch64-apple-darwin) arch=arm64 ;;
  x86_64-apple-darwin) arch=x86_64 ;;
  *) die "unsupported target $target; this script packages macOS targets only" ;;
esac

case "$optimize" in
  Debug | ReleaseSafe | ReleaseFast | ReleaseSmall) ;;
  *) die "unknown optimize mode $optimize" ;;
esac

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

prebuilt_dir=$root/third_party/libghostty-vt-sys/prebuilt/$target
target_dir=${CARGO_TARGET_DIR:-$root/target}/libghostty-prebuilt
build_dir=$target_dir/$target/debug/build

MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-13.0}
export MACOSX_DEPLOYMENT_TARGET

command -v zig >/dev/null 2>&1 || die "zig not found on PATH"
command -v cargo >/dev/null 2>&1 || die "cargo not found on PATH"

if command -v rustup >/dev/null 2>&1 &&
  ! rustup target list --installed | grep -qx "$target"; then
  die "Rust target $target is not installed; run: rustup target add $target"
fi

# A prebuilt link during the source build would package the old archive.
unset NMT_USE_PREBUILT_LIBGHOSTTY

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# 1. Build from source in a scratch target directory. Cleaning the package
#    there makes the build script run again, so the archive reflects the
#    current pin and patches even when nothing else changed.
echo "==> Building libghostty-vt from source (optimize=$optimize, target=$target)"

CARGO_TARGET_DIR=$target_dir cargo clean -p libghostty-vt-sys --target "$target" >/dev/null 2>&1 || true
CARGO_TARGET_DIR=$target_dir LIBGHOSTTY_VT_SYS_OPTIMIZE=$optimize \
  cargo build -p libghostty-vt-sys --target "$target"

# 2. The install prefix the build script just wrote. Cargo's build-directory
#    layout has changed between releases, so this searches rather than
#    assuming a path, and takes the newest match.
install=$(find "$build_dir" -type d -name ghostty-install -path '*libghostty-vt-sys*' \
  -exec stat -f '%m %N' {} + 2>/dev/null | sort -rn | head -n 1 | cut -d' ' -f2-)

[ -n "$install" ] || die "no ghostty-install directory under $build_dir"

archive=$install/lib/libghostty-vt.a
headers=$install/include

[ -f "$archive" ] || die "missing $archive"
[ -f "$headers/ghostty/vt.h" ] || die "missing $headers/ghostty/vt.h"

echo "    archive: $archive"
echo "    headers: $headers"

# 3. Strip debug information. It carries this machine's source paths and
#    would make every rebuild of the checked-in blob differ for no reason.
cp "$archive" "$work/libghostty-vt.a"
chmod u+w "$work/libghostty-vt.a"
strip -S "$work/libghostty-vt.a"
ranlib "$work/libghostty-vt.a"

# 4. Check what is about to be committed: the right architecture, an OS
#    minimum the app supports, and no paths from this machine.
archs=$(lipo -archs "$work/libghostty-vt.a" 2>/dev/null || lipo -info "$work/libghostty-vt.a")

echo "$archs" | tr ' ' '\n' | grep -qx "$arch" ||
  die "archive architectures are '$archs', expected $arch"

mkdir "$work/members"
(cd "$work/members" && ar x "$work/libghostty-vt.a" vt.o && chmod u+r vt.o)

minos=$(otool -l "$work/members/vt.o" | awk '/LC_BUILD_VERSION/ { found = 1 } found && $1 == "minos" { print $2; exit }')

[ -n "$minos" ] || die "could not read the minimum macOS version of vt.o"

newest=$(printf '%s\n%s\n' "$minos" "$MACOSX_DEPLOYMENT_TARGET" | sort -V | tail -n 1)

[ "$newest" = "$MACOSX_DEPLOYMENT_TARGET" ] ||
  die "archive requires macOS $minos, newer than the deployment target $MACOSX_DEPLOYMENT_TARGET"

if strings "$work/libghostty-vt.a" | grep -q "$HOME"; then
  echo "warning: the archive still mentions $HOME after stripping" >&2
fi

# 5. Replace the package. Headers are replaced as a whole so a header the
#    new sources dropped does not linger.
echo "==> Updating prebuilt package at $prebuilt_dir"

mkdir -p "$prebuilt_dir/lib" "$prebuilt_dir/include"
cp "$work/libghostty-vt.a" "$prebuilt_dir/lib/libghostty-vt.a"
rm -rf "$prebuilt_dir/include/ghostty"
cp -R "$headers/ghostty" "$prebuilt_dir/include/ghostty"

size=$(du -h "$prebuilt_dir/lib/libghostty-vt.a" | cut -f1)

echo "    lib/libghostty-vt.a: $size, $arch, macOS $minos minimum"

# 6. Link something against it. Building the -sys crate alone only proves the
#    files exist; a test binary has to resolve every symbol the bindings use.
if [ "$verify" = 1 ]; then
  echo "==> Verifying the prebuilt link path (NMT_USE_PREBUILT_LIBGHOSTTY=1)"

  CARGO_TARGET_DIR=$target_dir cargo clean -p libghostty-vt-sys --target "$target" >/dev/null 2>&1 || true
  CARGO_TARGET_DIR=$target_dir NMT_USE_PREBUILT_LIBGHOSTTY=1 \
    cargo test -p nmt_terminal --lib --no-run --target "$target"

  echo "    prebuilt link OK"
fi

echo
echo "Prebuilt updated. Review and commit:"
echo "  git add third_party/libghostty-vt-sys/prebuilt/$target"
echo "  git status --short -- third_party/libghostty-vt-sys/prebuilt/$target"
