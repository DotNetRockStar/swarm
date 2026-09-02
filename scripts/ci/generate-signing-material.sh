#!/usr/bin/env bash
# One-time: generate the signing material the release workflow needs and print
# the `gh secret set` commands to install it on this repository.
#
# Produces:
#   1. A self-signed macOS code-signing certificate (p12) for SWARM Server.
#      Not an Apple Developer ID — the app is NOT notarized. Its job is a
#      stable designated requirement so an in-place self-update keeps the
#      macOS file-access / TCC grants (server issue #196). A fresh .dmg
#      install still needs one right-click -> Open.
#   2. A Tauri updater keypair (minisign) for the server. The public half is
#      written into apps/server/tauri.conf.json; the private half is a secret.
#   3. An Android upload keystore for the Fire TV APK, so CI-signed builds
#      install as updates over an existing install.
#
# Re-running rotates everything. Rotating the macOS cert or the Android
# keystore means every already-installed copy must be reinstalled once.
# Rotating the updater key means shipping the new pubkey before old clients
# can verify new updates. Only rotate on purpose.
#
# Usage:  scripts/ci/generate-signing-material.sh [--print-only]

set -euo pipefail

REPO="SWARM-Media-Steaming/swarm"
REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PRINT_ONLY="${1:-}"

for tool in openssl gh npx keytool base64; do
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
CI=true npx --yes tauri signer generate --ci -p "$UPD_PASS" -w "$WORK/upd.key" >/dev/null 2>&1
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

echo
CMDS=$(cat <<EOF
gh secret set APPLE_CERTIFICATE          --repo $REPO --body '$CERT_B64'
gh secret set APPLE_CERTIFICATE_PASSWORD --repo $REPO --body '$CERT_PASS'
gh secret set APPLE_SIGNING_IDENTITY     --repo $REPO --body '$CERT_CN'
gh secret set SERVER_TAURI_SIGNING_PRIVATE_KEY          --repo $REPO --body '$UPD_KEY'
gh secret set SERVER_TAURI_SIGNING_PRIVATE_KEY_PASSWORD --repo $REPO --body '$UPD_PASS'
gh secret set SWARM_ANDROID_KEYSTORE_BASE64   --repo $REPO --body '$KS_B64'
gh secret set SWARM_ANDROID_KEYSTORE_PASSWORD --repo $REPO --body '$KS_PASS'
gh secret set SWARM_ANDROID_KEY_ALIAS         --repo $REPO --body '$KEY_ALIAS'
gh secret set SWARM_ANDROID_KEY_PASSWORD      --repo $REPO --body '$KEY_PASS'
EOF
)
echo "Run these to install the repository secrets:"
echo
echo "$CMDS"
echo

[ "$PRINT_ONLY" = "--print-only" ] && exit 0
read -r -p "Set these secrets on $REPO now? [y/N] " ANSWER
[ "$ANSWER" = "y" ] || [ "$ANSWER" = "Y" ] || { echo "Skipped. Copy the commands above when ready."; exit 0; }
eval "$CMDS"
echo "Secrets set. The next push to main will publish an updatable release."
