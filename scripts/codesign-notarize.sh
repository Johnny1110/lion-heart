#!/usr/bin/env bash
# Codesign and notarize the macOS artifacts. Requires (as env vars):
#   MACOS_CERTIFICATE            base64 of a Developer ID Application .p12
#   MACOS_CERTIFICATE_PASSWORD   its password
#   APPLE_ID                     the Apple ID that owns the certificate
#   APPLE_TEAM_ID                its team id
#   APPLE_APP_PASSWORD           an app-specific password for notarytool
#
# Locally: export the five variables and run scripts/codesign-notarize.sh
# after `cargo build --release -p lion-heart` and
# `cargo xtask bundle lion-heart-plugin --release`.
set -euo pipefail

IDENTITY_NAME="lion-heart-signing"
KEYCHAIN="lion-heart-signing.keychain-db"
KEYCHAIN_PASSWORD="$(openssl rand -hex 16)"

echo "== importing signing certificate into a throwaway keychain"
echo "$MACOS_CERTIFICATE" | base64 --decode > /tmp/certificate.p12
security create-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"
security default-keychain -s "$KEYCHAIN"
security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"
security import /tmp/certificate.p12 -k "$KEYCHAIN" \
  -P "$MACOS_CERTIFICATE_PASSWORD" -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple: -s \
  -k "$KEYCHAIN_PASSWORD" "$KEYCHAIN" > /dev/null
rm /tmp/certificate.p12
IDENTITY=$(security find-identity -v -p codesigning "$KEYCHAIN" \
  | awk -F'"' 'NR==1 {print $2}')
echo "== signing as: $IDENTITY"

sign() {
  codesign --force --deep --options runtime --timestamp \
    --sign "$IDENTITY" "$1"
}

notarize() {
  local archive="/tmp/$(basename "$1").zip"
  ditto -c -k --keepParent "$1" "$archive"
  xcrun notarytool submit "$archive" \
    --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" \
    --password "$APPLE_APP_PASSWORD" --wait
  # Bundles can be stapled; bare executables are validated online.
  xcrun stapler staple "$1" || true
  rm "$archive"
}

echo "== standalone binary"
sign target/release/lion-heart
notarize target/release/lion-heart

for bundle in target/bundled/*.clap target/bundled/*.vst3; do
  [ -e "$bundle" ] || continue
  echo "== $bundle"
  sign "$bundle"
  notarize "$bundle"
done

echo "== done"
