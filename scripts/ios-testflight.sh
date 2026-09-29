#!/bin/sh
# Build the iOS app for devices and upload it to App Store Connect, where
# TestFlight hands it to testers.
#
#   scripts/ios-testflight.sh             archive and upload
#   scripts/ios-testflight.sh --no-upload archive and export an .ipa only
#
# Needs:
#   - mobile/ios/Config/Local.xcconfig with NMT_DEVELOPMENT_TEAM and
#     NMT_BUNDLE_ID, or the same names in the environment; the bundle ID must
#     have an app record in App Store Connect.
#   - Signing that Xcode can manage: an Apple ID with access to the team
#     signed in to Xcode (Settings > Accounts). The first upload creates the
#     Apple Distribution certificate and the App Store profile, which takes
#     an Admin or Account Holder; later ones reuse them.
#   - Optionally an App Store Connect API key instead of the Xcode account:
#     NMT_ASC_KEY_PATH (the .p8), NMT_ASC_KEY_ID and NMT_ASC_ISSUER_ID.
#   - What scripts/build-ios-core.sh needs: the Rust iOS targets and Zig.
#     The archive runs it, building the core for release.
#
# The build number is the UTC time of the build, so every upload is newer
# than the last without any state to keep. The marketing version stays the
# project's MARKETING_VERSION; raising it starts a new TestFlight version,
# whose first build goes through Beta App Review again.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
ios="$root/mobile/ios"

upload=1

for arg in "$@"; do
    case $arg in
        --no-upload) upload=0 ;;
        *) echo "unknown option: $arg" >&2; exit 2 ;;
    esac
done

# The team is read the way Xcode reads it, from the environment first and
# then from Local.xcconfig, only to fail early with a useful message: a
# device archive without a team stops much later with a signing error.
team=${NMT_DEVELOPMENT_TEAM:-}
if [ -z "$team" ] && [ -f "$ios/Config/Local.xcconfig" ]; then
    team=$(sed -n 's/^[[:space:]]*NMT_DEVELOPMENT_TEAM[[:space:]]*=[[:space:]]*\([A-Z0-9]*\).*/\1/p' \
        "$ios/Config/Local.xcconfig")
fi
if [ -z "$team" ]; then
    echo "no team: set NMT_DEVELOPMENT_TEAM in mobile/ios/Config/Local.xcconfig" >&2
    exit 1
fi

auth=""
if [ -n "${NMT_ASC_KEY_PATH:-}" ]; then
    : "${NMT_ASC_KEY_ID:?NMT_ASC_KEY_ID is required with NMT_ASC_KEY_PATH}"
    : "${NMT_ASC_ISSUER_ID:?NMT_ASC_ISSUER_ID is required with NMT_ASC_KEY_PATH}"
    auth="-authenticationKeyPath $NMT_ASC_KEY_PATH -authenticationKeyID $NMT_ASC_KEY_ID -authenticationKeyIssuerID $NMT_ASC_ISSUER_ID"
fi

build=$(date -u +%Y%m%d%H%M)
work="$ios/build/testflight-$build"
archive="$work/NiumaTerm.xcarchive"

mkdir -p "$work"

# shellcheck disable=SC2086 # $auth is a list of options, empty without a key.
xcodebuild archive \
    -project "$ios/NiumaTerm.xcodeproj" \
    -scheme NiumaTerm \
    -configuration Release \
    -destination 'generic/platform=iOS' \
    -archivePath "$archive" \
    -allowProvisioningUpdates $auth \
    CURRENT_PROJECT_VERSION="$build"

if [ "$upload" = 1 ]; then
    destination=upload
else
    destination=export
fi

cat > "$work/ExportOptions.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>method</key>
    <string>app-store-connect</string>
    <key>destination</key>
    <string>$destination</string>
    <key>teamID</key>
    <string>$team</string>
    <key>signingStyle</key>
    <string>automatic</string>
    <key>manageAppVersionAndBuildNumber</key>
    <false/>
    <key>uploadSymbols</key>
    <true/>
</dict>
</plist>
EOF

# shellcheck disable=SC2086
xcodebuild -exportArchive \
    -archivePath "$archive" \
    -exportOptionsPlist "$work/ExportOptions.plist" \
    -exportPath "$work/export" \
    -allowProvisioningUpdates $auth

if [ "$upload" = 1 ]; then
    echo "uploaded build $build; it shows in TestFlight once App Store Connect finishes processing"
else
    echo "exported build $build to $work/export"
fi
