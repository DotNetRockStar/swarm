#!/usr/bin/env bash
# Sign "SWARM Server.app" with a STABLE local identity so macOS remembers
# the file-access grants the user gives it (GitHub #196).
#
# The problem: `tauri build` ad-hoc-signs the app (`codesign -s -`). An
# ad-hoc signature has no stable designated requirement, so macOS TCC keys
# its "allow files on a network volume / Files and Folders" decision on the
# main executable's cdhash. That hash changes on every rebuild, so each
# `install_media_server.sh` after a code change makes macOS forget and ask
# again — exactly the nag #196 is about.
#
# The fix: sign with a self-signed code-signing certificate kept in a
# DEDICATED keychain (never the login keychain, so there is never an
# interactive password/allow prompt). The designated requirement then pins
# `identifier "app.swarm.server" and certificate leaf = H"<sha1>"`, which is
# stable across rebuilds as long as the same certificate is reused. TCC
# grants — and a Full Disk Access entry the user adds by hand — then persist
# across reinstalls.
#
# Best-effort by design: if anything here fails (locked-down security
# tooling, no openssl, a managed Mac), the caller keeps the default ad-hoc
# signature and the app still runs; the user just gets re-prompted after
# updates, the pre-#196 behaviour.
#
# Usage: scripts/macos_stable_signing.sh "/Applications/SWARM Server.app"

set -euo pipefail

APP_PATH="${1:-}"
if [ -z "$APP_PATH" ] || [ ! -d "$APP_PATH" ]; then
    echo "macos_stable_signing: no app bundle at '${APP_PATH:-}'" >&2
    exit 1
fi
if [ "$(uname -s)" != "Darwin" ]; then
    echo "macos_stable_signing: macOS only" >&2
    exit 1
fi

BUNDLE_ID="app.swarm.server"
IDENTITY_CN="SWARM Server Local Signing"
KEYCHAIN_DIR="$HOME/Library/Keychains"
KEYCHAIN="$KEYCHAIN_DIR/swarm-local-signing.keychain-db"
# Fixed passphrase: this keychain only ever holds a self-signed code-signing
# key for this one local app. It is not a secret store.
KEYCHAIN_PASS="swarm-local-signing"

log() { echo "   $*"; }

ensure_identity() {
    if security find-identity -v -p codesigning "$KEYCHAIN" 2>/dev/null | grep -q "$IDENTITY_CN"; then
        return 0
    fi

    command -v openssl >/dev/null 2>&1 || { log "openssl not found — skipping stable signing"; return 1; }

    log "Creating the local signing identity '$IDENTITY_CN' (one time) ..."
    local work; work="$(mktemp -d)"
    trap 'rm -rf "$work"' RETURN

    # A code-signing leaf: basicConstraints CA:false + extendedKeyUsage
    # codeSigning is what `codesign` requires of a signing certificate.
    cat >"$work/openssl.cnf" <<'EOF'
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = SWARM Server Local Signing
[ext]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
EOF

    openssl req -x509 -newkey rsa:2048 -nodes \
        -keyout "$work/key.pem" -out "$work/cert.pem" \
        -days 7300 -config "$work/openssl.cnf" >/dev/null 2>&1 \
        || { log "certificate generation failed — skipping stable signing"; return 1; }

    openssl pkcs12 -export -inkey "$work/key.pem" -in "$work/cert.pem" \
        -out "$work/identity.p12" -passout "pass:$KEYCHAIN_PASS" -name "$IDENTITY_CN" >/dev/null 2>&1 \
        || { log "p12 packaging failed — skipping stable signing"; return 1; }

    mkdir -p "$KEYCHAIN_DIR"
    if [ ! -f "$KEYCHAIN" ]; then
        security create-keychain -p "$KEYCHAIN_PASS" "$KEYCHAIN"
    fi
    security set-keychain-settings "$KEYCHAIN"                       # no auto-lock timeout
    security unlock-keychain -p "$KEYCHAIN_PASS" "$KEYCHAIN"
    security import "$work/identity.p12" -k "$KEYCHAIN" -P "$KEYCHAIN_PASS" \
        -T /usr/bin/codesign -T /usr/bin/security >/dev/null

    # Let codesign use the key without a GUI prompt.
    security set-key-partition-list -S apple-tool:,apple: -s -k "$KEYCHAIN_PASS" "$KEYCHAIN" >/dev/null 2>&1 || true

    # Keep this keychain visible to the search list so `codesign -s` resolves
    # the identity by name (idempotent — de-dupes if already present).
    local list
    list="$(security list-keychains -d user | sed 's/[",]//g' | xargs)"
    case " $list " in
        *" $KEYCHAIN "*) : ;;
        *) security list-keychains -d user -s $list "$KEYCHAIN" ;;
    esac

    security find-identity -v -p codesigning "$KEYCHAIN" 2>/dev/null | grep -q "$IDENTITY_CN"
}

if ! ensure_identity; then
    echo "   Stable signing unavailable; keeping the ad-hoc signature."
    echo "   (macOS may re-ask for file access after future updates.)"
    exit 0
fi

security unlock-keychain -p "$KEYCHAIN_PASS" "$KEYCHAIN" 2>/dev/null || true

log "Signing $(basename "$APP_PATH") with the stable local identity ..."
if codesign --force --deep --keychain "$KEYCHAIN" \
        --identifier "$BUNDLE_ID" \
        --sign "$IDENTITY_CN" "$APP_PATH" >/dev/null 2>&1; then
    codesign --verify --deep --strict "$APP_PATH" >/dev/null 2>&1 \
        && log "Signed. macOS will remember file-access grants across updates." \
        || echo "   Signature verify failed; the app still runs (ad-hoc fallback behaviour)."
else
    echo "   codesign failed; keeping the ad-hoc signature."
fi
exit 0
