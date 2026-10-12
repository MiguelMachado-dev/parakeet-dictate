#!/bin/bash
# Builds "Parakeet Dictate.app" into mac/build, signed with a certificate made on this Mac, so macOS keeps
# the Microphone and Accessibility permissions across rebuilds. An ad-hoc signature changes with every
# build, and macOS then asks for the permissions again.
#
#   mac/scripts/build-app.sh            build and sign
#   mac/scripts/build-app.sh --install  also copy the app to /Applications and open it
#
# The certificate lives in its own keychain under ~/Library/Application Support/ParakeetDictate/signing,
# outside the login keychain. It signs nothing else and is trusted by nothing; delete that folder to remove it.
set -euo pipefail

MAC="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="$(dirname "$MAC")"
APP="$MAC/build/Parakeet Dictate.app"
SIGNING="$HOME/Library/Application Support/ParakeetDictate/signing"
KEYCHAIN="$SIGNING/signing.keychain-db"
NAME="Parakeet Dictate Local Signing"

make_identity() {
  local tmp search_list
  tmp="$(mktemp -d)"
  mkdir -p "$SIGNING"
  chmod 700 "$SIGNING"
  /usr/bin/openssl rand -hex 24 > "$SIGNING/password"
  chmod 600 "$SIGNING/password"
  cat > "$tmp/cert.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $NAME
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
EOF
  /usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 36500 -config "$tmp/cert.cnf" \
    -keyout "$tmp/key.pem" -out "$tmp/cert.pem" 2>/dev/null
  /usr/bin/openssl pkcs12 -export -inkey "$tmp/key.pem" -in "$tmp/cert.pem" -out "$tmp/id.p12" -passout pass:p
  # create-keychain adds the keychain to the user's search list; put the list back as it was.
  search_list="$(security list-keychains -d user | tr -d '"')"
  security create-keychain -p "$(cat "$SIGNING/password")" "$KEYCHAIN"
  # shellcheck disable=SC2086
  security list-keychains -d user -s $search_list
  security set-keychain-settings "$KEYCHAIN"
  security unlock-keychain -p "$(cat "$SIGNING/password")" "$KEYCHAIN"
  security import "$tmp/id.p12" -k "$KEYCHAIN" -P p -T /usr/bin/codesign >/dev/null
  security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$(cat "$SIGNING/password")" "$KEYCHAIN" >/dev/null
  rm -rf "$tmp"
  echo "Created the signing certificate \"$NAME\"."
}

make_icon() {
  local tmp
  tmp="$(mktemp -d)"
  mkdir "$tmp/AppIcon.iconset"
  sips -s format png "$ROOT/assets/parakeet.ico" --out "$tmp/icon.png" >/dev/null
  for size in 16 32 128 256; do
    sips -z $size $size "$tmp/icon.png" --out "$tmp/AppIcon.iconset/icon_${size}x${size}.png" >/dev/null
    if [[ $size -le 128 ]]; then
      sips -z $((size * 2)) $((size * 2)) "$tmp/icon.png" --out "$tmp/AppIcon.iconset/icon_${size}x${size}@2x.png" >/dev/null
    fi
  done
  iconutil -c icns "$tmp/AppIcon.iconset" -o "$APP/Contents/Resources/AppIcon.icns"
  rm -rf "$tmp"
}

cd "$MAC"
swift build -c release --product ParakeetDictate 2>&1 | grep -v "search path .* not found" || true
BINARY="$(swift build -c release --product ParakeetDictate --show-bin-path)/ParakeetDictate"
[[ -x "$BINARY" ]] || { echo "build failed" >&2; exit 1; }

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BINARY" "$APP/Contents/MacOS/ParakeetDictate"
cp "$MAC/Resources/Info.plist" "$APP/Contents/Info.plist"
make_icon

[[ -f "$KEYCHAIN" ]] || make_identity
security unlock-keychain -p "$(cat "$SIGNING/password")" "$KEYCHAIN"
IDENTITY="$(security find-certificate -c "$NAME" -Z "$KEYCHAIN" | awk '/SHA-1/ { print $NF }')"
codesign --force --sign "$IDENTITY" --keychain "$KEYCHAIN" "$APP"
codesign --verify "$APP"
echo "Built $APP"

if [[ "${1:-}" == "--install" ]]; then
  osascript -e 'quit app id "dev.miguelmachado.parakeet-dictate"' 2>/dev/null || true
  rm -rf "/Applications/Parakeet Dictate.app"
  cp -R "$APP" /Applications/
  open "/Applications/Parakeet Dictate.app"
  echo "Installed and opened /Applications/Parakeet Dictate.app"
fi
