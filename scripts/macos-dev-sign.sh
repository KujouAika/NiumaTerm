#!/bin/sh
# Sign a freshly built macOS binary with the Developer ID identity, then run it.
#
# The linker gives every build an ad-hoc signature, and macOS identifies an
# ad-hoc binary by its content hash. The Keychain remembers which program may
# read an item by that identity, so every rebuild looks like a new program and
# asks the user again for the remote-session sealing key. Signed with a
# certificate, the binary is identified by its name and team instead, which a
# rebuild keeps, so one "Always Allow" lasts.
#
# Cargo runs it as the runner for macOS targets (see .cargo/config.toml), so
# `cargo run` and `cargo test` binaries are signed before they start. It also
# works by hand for a binary launched directly:
#
#   scripts/macos-dev-sign.sh target/debug/NiumaTerm --testing
#
# Environment:
#   NMT_MACOS_SIGN_IDENTITY  signing identity; the keychain's only Developer ID
#                            Application identity when unset
#
# Without a usable identity the binary runs as built: signing only spares
# Keychain prompts, and a machine without the certificate must still build and
# test.
set -eu

binary=$1
shift

identity=${NMT_MACOS_SIGN_IDENTITY:-}
if [ -z "$identity" ]; then
  identity=$(security find-identity -v -p codesigning 2>/dev/null |
    sed -n 's/.*"\(Developer ID Application: .*\)"$/\1/p')
  # Two identities make the choice ambiguous; leave the build ad-hoc rather
  # than pick one silently.
  if [ "$(printf '%s' "$identity" | grep -c .)" -ne 1 ]; then
    identity=
  fi
fi

# A rebuild replaces the file and with it any earlier signature, so only an
# ad-hoc binary needs signing; skipping the rest keeps repeated runs instant.
if [ -n "$identity" ] &&
  codesign -dv "$binary" 2>&1 | grep -q '^Signature=adhoc$'; then
  # No timestamp: it needs Apple's server, and only distribution needs one.
  codesign --force --sign "$identity" --timestamp=none "$binary" 2>/dev/null ||
    echo "macos-dev-sign: signing failed; running $binary unsigned" >&2
fi

exec "$binary" "$@"
