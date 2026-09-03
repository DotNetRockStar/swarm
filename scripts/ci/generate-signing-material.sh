#!/usr/bin/env bash
# One-time: generate the signing material the release workflow needs and store
# it in this repository with `gh secret set`.
#
# Produces:
#   1. A self-signed macOS code-signing certificate (p12) for SWARM Server.
#      Not an Apple Developer ID — the app is NOT notarized. Its job is a
#      stable designated requirement so an in-place self-update keeps the
#      macOS file-access / TCC grants (server issue #196). A fresh .dmg
#      install still needs one right-click -> Open.
#   2. A Tauri updater keypair (minisign) for the server. The public half is
#      written into apps/server/tauri.conf.json; the private half is a secret.
#   3. An Android upload keystore for Fire TV APKs submitted to Amazon.
#
# Re-running rotates everything. Rotating the macOS cert can invalidate the
# identity of existing installs. Rotating the updater key means shipping the
# new pubkey before old clients can verify new updates. Keep the Android upload
# key stable for reproducible submission artifacts. Only rotate on purpose.
#
# Usage:  scripts/ci/generate-signing-material.sh [--yes|--print-only]

set -euo pipefail
trap 'echo "Signing setup failed at line $LINENO." >&2' ERR

REPO="SWARM-Media-Steaming/swarm"
REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
MODE="${1:-}"

if [ -n "$MODE" ] && [ "$MODE" != "--yes" ] && [ "$MODE" != "--print-only" ]; then
    echo "usage: $0 [--yes|--print-only]" >&2
    exit 2
fi

for tool in openssl gh npx keytool base64 jq; do
    command -v "$tool" >/dev/null || { echo "$tool is required" >&2; exit 1; }
done

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# ---- 1. macOS self-signed certificate -------------------------------------
CERT_PASS="$(openssl rand -hex 24)"
CERT_CN="SWARM Server CI Signing"
cat >"$WORK/openssl.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = ${CERT_CN}
[ext]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
EOF
openssl req -x509 -newkey rsa:2048 -nodes -keyout "$WORK/k.pem" -out "$WORK/c.pem" \
    -days 7300 -config "$WORK/openssl.cnf" >/dev/null 2>&1
openssl pkcs12 -export -inkey "$WORK/k.pem" -in "$WORK/c.pem" -out "$WORK/id.p12" \
    -passout "pass:${CERT_PASS}" -name "$CERT_CN" >/dev/null 2>&1
CERT_B64="$(base64 < "$WORK/id.p12" | tr -d '\n')"

# ---- 2. Server updater keypair -------------------------------------------
UPD_PASS="$(openssl rand -hex 24)"
CI=true npx --yes @tauri-apps/cli signer generate --ci -p "$UPD_PASS" -w "$WORK/upd.key" >/dev/null 2>&1
UPD_KEY="$(cat "$WORK/upd.key")"
UPD_PUB="$(cat "$WORK/upd.key.pub")"
CONF="$REPO_ROOT/apps/server/tauri.conf.json"
tmp="$(mktemp)"
jq --arg k "$UPD_PUB" '.plugins.updater.pubkey = $k' "$CONF" >"$tmp" && mv "$tmp" "$CONF"
echo "Updated apps/server/tauri.conf.json (plugins.updater.pubkey) — commit this change."

# ---- 3. Android upload keystore -----------------------------------------
KS_PASS="$(openssl rand -hex 24)"
KEY_PASS="$(openssl rand -hex 24)"
KEY_ALIAS="swarm-tv"
keytool -genkeypair -v -keystore "$WORK/upload.keystore" -alias "$KEY_ALIAS" \
    -keyalg RSA -keysize 2048 -validity 10000 \
    -storepass "$KS_PASS" -keypass "$KEY_PASS" \
    -dname "CN=SWARM Fire TV, O=SWARM, C=US" >/dev/null 2>&1
KS_B64="$(base64 < "$WORK/upload.keystore" | tr -d '\n')"

if [ "$MODE" = "--print-only" ]; then
    echo "Generated signing material without storing secrets."
    exit 0
fi

if [ "$MODE" != "--yes" ]; then
    read -r -p "Set the generated secrets on $REPO now? [y/N] " ANSWER
    [ "$ANSWER" = "y" ] || [ "$ANSWER" = "Y" ] || { echo "Skipped."; exit 0; }
fi

# Pass values over stdin so private material is neither printed nor exposed in
# process arguments. `--yes` makes the one-time setup safe for automation.
printf '%s' "$CERT_B64" | gh secret set APPLE_CERTIFICATE --repo "$REPO"
printf '%s' "$CERT_PASS" | gh secret set APPLE_CERTIFICATE_PASSWORD --repo "$REPO"
printf '%s' "$CERT_CN" | gh secret set APPLE_SIGNING_IDENTITY --repo "$REPO"
printf '%s' "$UPD_KEY" | gh secret set SERVER_TAURI_SIGNING_PRIVATE_KEY --repo "$REPO"
printf '%s' "$UPD_PASS" | gh secret set SERVER_TAURI_SIGNING_PRIVATE_KEY_PASSWORD --repo "$REPO"
printf '%s' "$KS_B64" | gh secret set SWARM_ANDROID_KEYSTORE_BASE64 --repo "$REPO"
printf '%s' "$KS_PASS" | gh secret set SWARM_ANDROID_KEYSTORE_PASSWORD --repo "$REPO"
printf '%s' "$KEY_ALIAS" | gh secret set SWARM_ANDROID_KEY_ALIAS --repo "$REPO"
printf '%s' "$KEY_PASS" | gh secret set SWARM_ANDROID_KEY_PASSWORD --repo "$REPO"
echo "Secrets set. The next push to main will publish an updatable release."
