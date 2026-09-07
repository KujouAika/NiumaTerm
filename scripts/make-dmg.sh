#!/bin/sh
# Build NiumaTerm.dmg: the app, a shortcut to /Applications, and a window that
# shows what to do with them.
#
# The window layout lives in the `.DS_Store` Finder writes, and only Finder can
# write one. Driving Finder needs an automation grant that nobody can approve on
# a build runner, so the layout is recorded once by hand into
# `assets/macos/dmg-DS_Store` and copied in from there. `--arrange` is what
# re-records it, and is meant to be run on a desktop after the background or the
# icon positions change.
#
# Usage:
#   scripts/make-dmg.sh <NiumaTerm.app> <out.dmg>   build from the recorded layout
#   scripts/make-dmg.sh --arrange <NiumaTerm.app>   re-record the layout
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
volume=NiumaTerm
layout=$root/assets/macos/dmg-DS_Store

arrange=no
if [ "${1:-}" = "--arrange" ]; then
  arrange=yes
  shift
fi

app=${1:?usage: make-dmg.sh [--arrange] <NiumaTerm.app> [out.dmg]}
out=${2:-}
if [ "$arrange" = no ] && [ -z "$out" ]; then
  echo "usage: make-dmg.sh <NiumaTerm.app> <out.dmg>" >&2
  exit 2
fi

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/.background"

# Both scales in one file: a disk image background is a fixed bitmap that
# nothing rescales, so the 1x image alone is soft on a Retina display. Built
# here rather than kept in the tree, since it is derived from the two PNGs.
tiffutil -cathidpicheck \
  "$root/assets/macos/dmg-background.png" \
  "$root/assets/macos/dmg-background@2x.png" \
  -out "$stage/.background/background.tiff" >/dev/null
# ditto rather than cp -R: the app carries a code signature whose validity
# depends on extended attributes surviving the copy.
ditto "$app" "$stage/NiumaTerm.app"
ln -s /Applications "$stage/Applications"

if [ "$arrange" = no ]; then
  [ -f "$layout" ] || { echo "no recorded layout at $layout; run --arrange first" >&2; exit 1; }
  cp "$layout" "$stage/.DS_Store"
  rm -f "$out"
  hdiutil create -srcfolder "$stage" -volname "$volume" -fs APFS \
    -format UDZO -imagekey zlib-level=9 "$out" >/dev/null
  echo "$out"
  exit 0
fi

# Recording the layout. The image is built at a fixed, unremarkable path because
# the alias Finder stores for the background remembers where the image lived,
# and that string ends up in a file this repository keeps.
work=/tmp/NiumaTerm-dmg
rm -rf "$work"
mkdir -p "$work"
rw=$work/rw.dmg
hdiutil create -srcfolder "$stage" -volname "$volume" -fs APFS \
  -format UDRW -size 200m "$rw" >/dev/null

device=$(hdiutil attach -readwrite -noverify -noautoopen "$rw" | grep -o '/dev/disk[0-9]*' | head -1)
mount=/Volumes/$volume

# Finder needs the window open before its view options exist, and it writes the
# result asynchronously; the reopen is what commits it.
osascript <<APPLESCRIPT
tell application "Finder"
    tell disk "$volume"
        open
        set current view of container window to icon view
        set toolbar visible of container window to false
        set statusbar visible of container window to false
        -- Finder counts the title bar inside these bounds, so the height carries
        -- it on top of the background's own 400 points; without that the image
        -- is clipped exactly where the instruction sits.
        set the bounds of container window to {200, 120, 860, 546}
        set options to the icon view options of container window
        set arrangement of options to not arranged
        set icon size of options to 128
        set text size of options to 13
        -- A POSIX path leaves Finder nothing to resolve. The colon-separated
        -- form is taken relative to something else and silently sets nothing.
        set background picture of options to POSIX file "$mount/.background/background.tiff"
        set position of item "NiumaTerm.app" of container window to {165, 155}
        set position of item "Applications" of container window to {495, 155}
        close
        open
        update without registering applications
        delay 3
    end tell
end tell
APPLESCRIPT

# Finder accepts the background but cannot report the property back, so what
# gets checked is the arrangement it wrote.
if ! strings "$mount/.DS_Store" 2>/dev/null | grep -q backgroundImageAlias; then
  echo "the window has no background picture; the arrangement did not take" >&2
  hdiutil detach "$device" >/dev/null 2>&1 || true
  exit 1
fi

cp "$mount/.DS_Store" "$layout"
sync
hdiutil detach "$device" >/dev/null
rm -rf "$work"

echo "recorded $layout"
