#!/usr/bin/env bash
# Build Server Hub on your Mac, sign it with your Developer ID, notarize it with Apple and upload it to the GitHub release
# of the version in package.json (replaces the unsigned .dmg built by CI).
#   ./scripts/release-signed.sh
# Needs: a "Developer ID Application" certificate in your keychain, Xcode command line tools, Node, Rust, gh (logged in).
# The first run asks once for your Apple ID + an app-specific password (appleid.apple.com) and keeps them in the keychain.
set -euo pipefail
cd "$(dirname "$0")/.."

REPO=nvminhtu/server-hub
VERSION=$(node -p "require('./package.json').version")
TAG="v$VERSION"
PROFILE="${NOTARY_PROFILE:-server-hub}"
APP="src-tauri/target/universal-apple-darwin/release/bundle/macos/Server Hub.app"
DMG=Server-Hub-mac.dmg

IDENTITY="${APPLE_SIGNING_IDENTITY:-$(security find-identity -v -p codesigning | awk -F'"' '/Developer ID Application/ {print $2; exit}')}"
if [ -z "$IDENTITY" ]; then
  echo "✗ No 'Developer ID Application' certificate found in your keychain (security find-identity -v -p codesigning)." >&2
  exit 1
fi
TEAM=$(sed -n 's/.*(\([A-Z0-9]*\))$/\1/p' <<<"$IDENTITY")
echo "→ Signing $TAG as: $IDENTITY"

if ! xcrun notarytool history --keychain-profile "$PROFILE" >/dev/null 2>&1; then
  echo "→ One-time setup: save your notarization login in the keychain (profile \"$PROFILE\")."
  echo "  Apple ID = your developer account email · password = an app-specific password from appleid.apple.com"
  xcrun notarytool store-credentials "$PROFILE" --team-id "$TEAM"
fi

echo "→ Build"
rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
npm ci
npm run check
# tauri signs the app (hardened runtime) with this identity; notarization is done below with the keychain profile
env -u APPLE_ID -u APPLE_PASSWORD -u APPLE_API_KEY -u APPLE_API_ISSUER -u APPLE_CERTIFICATE \
  APPLE_SIGNING_IDENTITY="$IDENTITY" npx tauri build --target universal-apple-darwin --bundles dmg
codesign --verify --deep --strict --verbose=1 "$APP"

cp src-tauri/target/universal-apple-darwin/release/bundle/dmg/*.dmg "$DMG"
codesign --force --sign "$IDENTITY" --timestamp "$DMG"

echo "→ Notarize (Apple usually answers in 1–10 minutes)"
xcrun notarytool submit "$DMG" --keychain-profile "$PROFILE" --wait | tee notary.log
grep -q "status: Accepted" notary.log || { echo "✗ Notarization was not accepted — see: xcrun notarytool log <id> --keychain-profile $PROFILE" >&2; exit 1; }
xcrun stapler staple "$DMG"
spctl -a -t open --context context:primary-signature -v "$DMG"
shasum -a 256 "$DMG" > SHA256SUMS.txt
rm -f notary.log

echo "→ Upload to $TAG"
export GH_TOKEN="${GH_TOKEN:-$(gh auth token -u nvminhtu 2>/dev/null || gh auth token)}"
if gh release view "$TAG" -R "$REPO" >/dev/null 2>&1; then
  gh release upload "$TAG" "$DMG" SHA256SUMS.txt --clobber -R "$REPO"
else
  gh release create "$TAG" "$DMG" SHA256SUMS.txt -R "$REPO" --title "Server Hub $VERSION" --notes "Server Hub $VERSION"
fi
echo "✓ Signed + notarized $DMG is live: https://github.com/$REPO/releases/tag/$TAG"
cat SHA256SUMS.txt
