//! rustls certificate verifiers that authenticate peers by certificate
//! fingerprint alone — no CA, no hostname, exactly the Batocera.Drone trust
//! model: "the pinned fingerprint set is the only authority."
//!
//! The connecting side pins exactly one expected fingerprint (it knows which
//! device it is dialing). The accepting side holds a shared, updatable set of
//! allowed fingerprints (the swarm roster) and requires a client certificate
//! whose digest is in it. TLS signature verification still runs — the
//! handshake proves possession of the pinned certificate's private key.

use crate::identity::fingerprint_der;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{ring, verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, Error as TlsError, SignatureScheme};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

/// Live set of fingerprints allowed to connect — the accepting side's swarm
/// roster. Cloneable handle; updates apply to accepted connections' checks
/// immediately.
#[derive(Clone, Default)]
pub struct AllowedPeers {
    inner: Arc<RwLock<AllowedPeerSets>>,
}

#[derive(Default)]
struct AllowedPeerSets {
    persistent: HashSet<String>,
    /// Short-lived grants are keyed by an activation/run id so expiry of an
    /// older grant cannot accidentally revoke a newer grant for the same
    /// certificate. They are intentionally kept separate from [persistent]
    /// so roster refreshes cannot erase a live test session and test cleanup
    /// cannot erase a real saved pairing.
    ephemeral: HashMap<String, HashSet<String>>,
}

impl AllowedPeers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn replace(&self, fingerprints: impl IntoIterator<Item = String>) {
        self.inner.write().unwrap().persistent = fingerprints.into_iter().collect();
    }

    pub fn insert(&self, fingerprint: &str) {
        self.inner
            .write()
            .unwrap()
            .persistent
            .insert(fingerprint.to_string());
    }

    pub fn remove(&self, fingerprint: &str) {
        self.inner.write().unwrap().persistent.remove(fingerprint);
    }

    /// Add one explicitly-scoped, non-persistent authorization grant.
    pub fn insert_ephemeral(&self, fingerprint: &str, grant_id: &str) {
        self.inner
            .write()
            .unwrap()
            .ephemeral
            .entry(fingerprint.to_string())
            .or_default()
            .insert(grant_id.to_string());
    }

    /// Revoke exactly one ephemeral grant without disturbing permanent trust
    /// or another overlapping ephemeral grant for the same certificate.
    pub fn remove_ephemeral(&self, fingerprint: &str, grant_id: &str) {
        let mut inner = self.inner.write().unwrap();
        let remove_fingerprint = inner
            .ephemeral
            .get_mut(fingerprint)
            .map(|grants| {
                grants.remove(grant_id);
                grants.is_empty()
            })
            .unwrap_or(false);
        if remove_fingerprint {
            inner.ephemeral.remove(fingerprint);
        }
    }

    pub fn contains(&self, fingerprint: &str) -> bool {
        let inner = self.inner.read().unwrap();
        inner.persistent.contains(fingerprint) || inner.ephemeral.contains_key(fingerprint)
    }

    pub fn len(&self) -> usize {
        let inner = self.inner.read().unwrap();
        inner
            .persistent
            .iter()
            .chain(inner.ephemeral.keys())
            .collect::<HashSet<_>>()
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// A point-in-time copy of the set, for status/diagnostics display.
    pub fn snapshot(&self) -> HashSet<String> {
        let inner = self.inner.read().unwrap();
        inner
            .persistent
            .iter()
            .cloned()
            .chain(inner.ephemeral.keys().cloned())
            .collect()
    }
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(ring::default_provider())
}

/// Client-side verifier: accept exactly one server certificate — the one
/// whose SHA-256 matches the pin learned from the STUN roster/signaling.
#[derive(Debug)]
pub struct PinnedServerVerifier {
    expected_fingerprint: String,
    provider: Arc<CryptoProvider>,
}

impl PinnedServerVerifier {
    pub fn new(expected_fingerprint: &str) -> Arc<Self> {
        Arc::new(Self {
            expected_fingerprint: expected_fingerprint.to_lowercase(),
            provider: provider(),
        })
    }
}

impl ServerCertVerifier for PinnedServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        if fingerprint_der(end_entity) == self.expected_fingerprint {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(TlsError::General(
                "server certificate does not match pinned fingerprint".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Server-side verifier: require a client certificate whose SHA-256 is in the
/// allowed set. Membership is re-checked per handshake against the live set.
#[derive(Debug)]
pub struct RosterClientVerifier {
    allowed: AllowedPeers,
    provider: Arc<CryptoProvider>,
}

impl std::fmt::Debug for AllowedPeers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AllowedPeers")
            .field("count", &self.len())
            .finish()
    }
}

impl RosterClientVerifier {
    pub fn new(allowed: AllowedPeers) -> Arc<Self> {
        Arc::new(Self {
            allowed,
            provider: provider(),
        })
    }
}

impl ClientCertVerifier for RosterClientVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, TlsError> {
        if self.allowed.contains(&fingerprint_der(end_entity)) {
            Ok(ClientCertVerified::assertion())
        } else {
            Err(TlsError::General(
                "client certificate is not in the allowed peer set".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::AllowedPeers;

    #[test]
    fn ephemeral_grants_survive_roster_replace_and_expire_independently() {
        let allowed = AllowedPeers::new();
        allowed.replace(["permanent".to_string()]);
        allowed.insert_ephemeral("testing", "run-a");
        allowed.insert_ephemeral("testing", "run-b");

        allowed.replace(["replacement".to_string()]);
        assert!(!allowed.contains("permanent"));
        assert!(allowed.contains("replacement"));
        assert!(allowed.contains("testing"));

        allowed.remove_ephemeral("testing", "run-a");
        assert!(allowed.contains("testing"));
        allowed.remove_ephemeral("testing", "run-b");
        assert!(!allowed.contains("testing"));
    }

    #[test]
    fn ephemeral_cleanup_never_revokes_persistent_trust() {
        let allowed = AllowedPeers::new();
        allowed.insert("same");
        allowed.insert_ephemeral("same", "test-run");
        assert_eq!(allowed.len(), 1);

        allowed.remove_ephemeral("same", "test-run");
        assert!(allowed.contains("same"));
        assert_eq!(
            allowed.snapshot(),
            ["same".to_string()].into_iter().collect()
        );
    }
}
