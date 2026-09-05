//! A second, independent certificate authority for the plain-HTTPS media
//! surface (`apps/server/src/http_media.rs`) — deliberately **not** the QUIC
//! peer identity in `identity.rs`.
//!
//! The identity cert is a bare self-signed leaf with no `BasicConstraints
//! CA:TRUE` — it is a pinned end-entity certificate, not a signing
//! authority, and regenerating it "breaks every existing pin by design" per
//! its own doc comment. It cannot be reused to sign anything a strict TLS
//! client will accept, and it must never be touched by this module.
//!
//! Instead this module owns a **separate, long-lived CA** (`ensure_http_ca`,
//! same reuse-if-present persistence pattern as `identity::ensure_identity`
//! so its fingerprint is stable across restarts) and issues **short-lived
//! leaves** from it (`issue_http_leaf`) carrying SANs for whatever
//! hostnames/IPs the HTTPS listener needs to answer for. An HTTP-only
//! client (Roku) is handed the CA's PEM once, at pairing time
//! (`/pair/poll`'s `http_ca_pem` field), and trusts it as its sole root —
//! Roku's `Video`/`roUrlTransfer` APIs expose no way to disable hostname
//! verification, so presenting a CA-signed leaf with real SANs is required,
//! not a nicety; a bare self-signed leaf would not validate.
//!
//! Leaves are reissued (not cached) on every call — SANs change whenever
//! the server's local IP or relay hostname changes, and re-signing is cheap
//! (no I/O, no persistence) compared to tracking invalidation.

use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use time::{Duration, OffsetDateTime};

#[derive(Debug, thiserror::Error)]
pub enum HttpTlsError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("certificate generation failed: {0}")]
    Generate(#[from] rcgen::Error),
    #[error("stored HTTP CA is corrupt: {0}")]
    Corrupt(String),
}

/// The long-lived signing authority for the HTTP media surface's leaves.
/// Not a QUIC peer identity — never pinned by a peer, never submitted to
/// the STUN server.
pub struct HttpCa {
    cert: rcgen::Certificate,
    key: rcgen::KeyPair,
    /// PEM of `cert`, handed to a pairing client as its trust root.
    pub cert_pem: String,
    /// Lowercase hex SHA-256 of the CA certificate's DER — for logging only,
    /// never pinned by a client (the client trusts the whole CA, not one
    /// fingerprint, since leaves are reissued freely).
    pub fingerprint: String,
}

impl std::fmt::Debug for HttpCa {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpCa")
            .field("fingerprint", &self.fingerprint)
            .finish()
    }
}

/// A freshly issued, short-lived leaf certificate + key ready to hand to a
/// `rustls::ServerConfig`.
pub struct HttpLeaf {
    pub cert_der: CertificateDer<'static>,
    pub key_der: PrivateKeyDer<'static>,
}

fn ca_cert_path(dir: &Path) -> PathBuf {
    dir.join("http-ca-cert.pem")
}

fn ca_key_path(dir: &Path) -> PathBuf {
    dir.join("http-ca-key.pem")
}

/// Load the persisted HTTP CA from `dir`, or generate and persist a new one.
/// Reuse-if-present keeps the CA (and therefore every client that has
/// already trusted it) stable across restarts — regenerating would silently
/// untrust every previously paired HTTP-only device.
pub fn ensure_http_ca(dir: &Path) -> Result<HttpCa, HttpTlsError> {
    std::fs::create_dir_all(dir)?;
    let cert_file = ca_cert_path(dir);
    let key_file = ca_key_path(dir);
    if cert_file.exists() && key_file.exists() {
        return load_ca(&cert_file, &key_file);
    }

    let mut params = rcgen::CertificateParams::new(Vec::<String>::new())?;
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::DigitalSignature,
        rcgen::KeyUsagePurpose::CrlSign,
    ];
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "SWARM HTTP Media CA");
    // Long-lived: this is a locally-generated, single-purpose root trusted
    // by exactly the devices this one server pairs — there is no
    // cross-server chain to keep short-lived for, unlike a leaf.
    params.not_before = OffsetDateTime::now_utc() - Duration::days(1);
    params.not_after = OffsetDateTime::now_utc() + Duration::days(365 * 20);

    let key = rcgen::KeyPair::generate()?;
    let cert = params.self_signed(&key)?;
    std::fs::write(&cert_file, cert.pem())?;
    std::fs::write(&key_file, key.serialize_pem())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o600))?;
    }
    load_ca(&cert_file, &key_file)
}

fn load_ca(cert_file: &Path, key_file: &Path) -> Result<HttpCa, HttpTlsError> {
    let cert_pem = std::fs::read_to_string(cert_file)?;
    let key_pem = std::fs::read_to_string(key_file)?;
    let key = rcgen::KeyPair::from_pem(&key_pem)
        .map_err(|e| HttpTlsError::Corrupt(format!("{} is not a valid key pair: {e}", key_file.display())))?;
    let params = rcgen::CertificateParams::from_ca_cert_pem(&cert_pem).map_err(|e| {
        HttpTlsError::Corrupt(format!("{} is not a valid CA certificate: {e}", cert_file.display()))
    })?;
    let cert = params.self_signed(&key)?;
    // Fingerprint the *key*, not `cert.der()`: `self_signed` re-signs on
    // every load (ECDSA signatures are randomized), so the reconstructed
    // certificate's DER — and therefore its hash — differs on every call
    // even though it represents the same logical CA. The public key is the
    // part that's actually stable across reloads, and is what a client's
    // trust in `cert_pem` (the literal, byte-stable persisted PEM) really
    // rests on.
    let fingerprint = hex::encode(Sha256::digest(key.public_key_der()));
    Ok(HttpCa {
        cert,
        key,
        cert_pem,
        fingerprint,
    })
}

/// Issue a fresh short-lived leaf signed by `ca`, valid for the given SANs
/// (a mix of IPv4/IPv6 literals and DNS names — `rcgen` classifies each
/// string automatically). Not persisted: call again whenever the set of
/// SANs the listener must answer for changes (a new local IP, a newly
/// granted relay hostname).
pub fn issue_http_leaf(ca: &HttpCa, sans: Vec<String>) -> Result<HttpLeaf, HttpTlsError> {
    let mut params = rcgen::CertificateParams::new(sans)?;
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "swarm-http-media");
    params.is_ca = rcgen::IsCa::NoCa;
    params.key_usages = vec![
        rcgen::KeyUsagePurpose::DigitalSignature,
        rcgen::KeyUsagePurpose::KeyEncipherment,
    ];
    params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
    // Short-lived by design: reissued fresh at every server startup (and
    // whenever the SAN set changes), so there is no revocation story to
    // build — an expired/stale leaf just gets replaced next start.
    params.not_before = OffsetDateTime::now_utc() - Duration::hours(1);
    params.not_after = OffsetDateTime::now_utc() + Duration::days(30);

    let leaf_key = rcgen::KeyPair::generate()?;
    let leaf_cert = params.signed_by(&leaf_key, &ca.cert, &ca.key)?;

    Ok(HttpLeaf {
        cert_der: leaf_cert.der().clone(),
        key_der: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ca_persists_and_reuses_fingerprint_across_reload() {
        let dir = tempfile::tempdir().unwrap();
        let first = ensure_http_ca(dir.path()).unwrap();
        let second = ensure_http_ca(dir.path()).unwrap();
        assert_eq!(first.fingerprint, second.fingerprint);
        assert_eq!(first.cert_pem, second.cert_pem);
    }

    #[test]
    fn issued_leaf_is_signed_by_the_ca_and_carries_requested_sans() {
        let dir = tempfile::tempdir().unwrap();
        let ca = ensure_http_ca(dir.path()).unwrap();
        let leaf = issue_http_leaf(
            &ca,
            vec!["192.168.1.50".to_string(), "swarm-relay.example.org".to_string()],
        )
        .unwrap();

        // Round-trip the leaf through a real rustls verifier against the CA
        // as the sole trust anchor -- this is the actual contract a Roku
        // client's HTTPCertificatesFile relies on, not just "rcgen didn't
        // error."
        let mut roots = rustls::RootCertStore::empty();
        roots.add(ca.cert.der().clone()).unwrap();
        let verifier = rustls::client::WebPkiServerVerifier::builder(std::sync::Arc::new(roots))
            .build()
            .unwrap();
        use rustls::client::danger::ServerCertVerifier;
        use rustls_pki_types::ServerName;
        let server_name = ServerName::try_from("192.168.1.50").unwrap();
        verifier
            .verify_server_cert(
                &leaf.cert_der,
                &[],
                &server_name,
                &[],
                rustls_pki_types::UnixTime::now(),
            )
            .expect("leaf must chain-validate against the issuing CA for the requested SAN");
    }

    #[test]
    fn two_ca_instances_in_different_dirs_have_different_fingerprints() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let a = ensure_http_ca(dir_a.path()).unwrap();
        let b = ensure_http_ca(dir_b.path()).unwrap();
        assert_ne!(a.fingerprint, b.fingerprint);
    }
}
