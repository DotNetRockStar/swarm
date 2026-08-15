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
use std::collections::HashSet;
use std::sync::{Arc, RwLock};

/// Live set of fingerprints allowed to connect — the accepting side's swarm
/// roster. Cloneable handle; updates apply to accepted connections' checks
/// immediately.
#[derive(Clone, Default)]
pub struct AllowedPeers {
    inner: Arc<RwLock<HashSet<String>>>,
}

impl AllowedPeers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn replace(&self, fingerprints: impl IntoIterator<Item = String>) {
        *self.inner.write().unwrap() = fingerprints.into_iter().collect();
    }

    pub fn insert(&self, fingerprint: &str) {
        self.inner.write().unwrap().insert(fingerprint.to_string());
    }

    pub fn remove(&self, fingerprint: &str) {
        self.inner.write().unwrap().remove(fingerprint);
    }

    pub fn contains(&self, fingerprint: &str) -> bool {
        self.inner.read().unwrap().contains(fingerprint)
    }

    pub fn len(&self) -> usize {
        self.inner.read().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// A point-in-time copy of the set, for status/diagnostics display.
    pub fn snapshot(&self) -> HashSet<String> {
        self.inner.read().unwrap().clone()
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
        Arc::new(Self { expected_fingerprint: expected_fingerprint.to_lowercase(), provider: provider() })
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
            Err(TlsError::General("server certificate does not match pinned fingerprint".into()))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
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
        f.debug_struct("AllowedPeers").field("count", &self.inner.read().unwrap().len()).finish()
    }
}

impl RosterClientVerifier {
    pub fn new(allowed: AllowedPeers) -> Arc<Self> {
        Arc::new(Self { allowed, provider: provider() })
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
            Err(TlsError::General("client certificate is not in the allowed peer set".into()))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}
