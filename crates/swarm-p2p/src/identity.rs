//! Device identity: a long-lived self-signed certificate whose private key
//! never leaves the device. The SHA-256 of the DER certificate is the
//! device's peer identity — submitted to the STUN server at registration
//! (the TOFU moment) and pinned by peers on every QUIC handshake.
//!
//! Port of Batocera.Drone's `DroneCertificateManager` without the openssl
//! subprocess: rcgen generates the cert, PEM files persist it (0600 on the
//! key), and reuse-if-present keeps the fingerprint stable across restarts —
//! regenerating breaks every existing pin by design.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("certificate generation failed: {0}")]
    Generate(#[from] rcgen::Error),
    #[error("stored identity is corrupt: {0}")]
    Corrupt(String),
}

#[derive(Clone)]
pub struct DeviceIdentity {
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
    /// Lowercase hex SHA-256 of `cert_der`.
    pub fingerprint: String,
}

impl std::fmt::Debug for DeviceIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceIdentity")
            .field("fingerprint", &self.fingerprint)
            .finish()
    }
}

pub fn fingerprint_der(cert_der: &[u8]) -> String {
    hex::encode(Sha256::digest(cert_der))
}

fn cert_path(dir: &Path) -> PathBuf {
    dir.join("device-cert.pem")
}

fn key_path(dir: &Path) -> PathBuf {
    dir.join("device-key.pem")
}

/// Load the persisted identity from `dir`, or generate and persist a new one.
pub fn ensure_identity(dir: &Path) -> Result<DeviceIdentity, IdentityError> {
    std::fs::create_dir_all(dir)?;
    let cert_file = cert_path(dir);
    let key_file = key_path(dir);
    if cert_file.exists() && key_file.exists() {
        return load(&cert_file, &key_file);
    }

    let rcgen::CertifiedKey { cert, key_pair } =
        rcgen::generate_simple_self_signed(vec!["swarm-device".to_string()])?;
    std::fs::write(&cert_file, cert.pem())?;
    std::fs::write(&key_file, key_pair.serialize_pem())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o600))?;
    }
    load(&cert_file, &key_file)
}

fn load(cert_file: &Path, key_file: &Path) -> Result<DeviceIdentity, IdentityError> {
    let cert_der =
        pem_to_der(&std::fs::read_to_string(cert_file)?, "CERTIFICATE").ok_or_else(|| {
            IdentityError::Corrupt(format!("{} is not a PEM certificate", cert_file.display()))
        })?;
    let key_pem = std::fs::read_to_string(key_file)?;
    let key_der = pem_to_der(&key_pem, "PRIVATE KEY").ok_or_else(|| {
        IdentityError::Corrupt(format!("{} is not a PEM private key", key_file.display()))
    })?;
    let fingerprint = fingerprint_der(&cert_der);
    Ok(DeviceIdentity {
        cert_der,
        key_der,
        fingerprint,
    })
}

/// Minimal PEM decoder for the two block types we write ourselves.
fn pem_to_der(pem: &str, label: &str) -> Option<Vec<u8>> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let body: String = pem
        .split_once(&begin)?
        .1
        .split_once(&end)?
        .0
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    base64_decode(&body)
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut lookup = [255u8; 256];
    for (i, &c) in TABLE.iter().enumerate() {
        lookup[c as usize] = i as u8;
    }
    let bytes: Vec<u8> = input.bytes().filter(|&b| b != b'=').collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut acc: u32 = 0;
        for &b in chunk {
            let v = lookup[b as usize];
            if v == 255 {
                return None;
            }
            acc = (acc << 6) | v as u32;
        }
        match chunk.len() {
            4 => out.extend_from_slice(&[(acc >> 16) as u8, (acc >> 8) as u8, acc as u8]),
            3 => {
                let acc = acc << 6;
                out.extend_from_slice(&[(acc >> 16) as u8, (acc >> 8) as u8]);
            }
            2 => {
                let acc = acc << 12;
                out.push((acc >> 16) as u8);
            }
            _ => return None,
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("swarm-id-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn identity_is_stable_across_reloads() {
        let dir = temp_dir("stable");
        let first = ensure_identity(&dir).unwrap();
        let second = ensure_identity(&dir).unwrap();
        assert_eq!(first.fingerprint, second.fingerprint);
        assert_eq!(first.cert_der, second.cert_der);
        assert_eq!(first.fingerprint.len(), 64);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn distinct_dirs_get_distinct_identities() {
        let dir_a = temp_dir("a");
        let dir_b = temp_dir("b");
        let a = ensure_identity(&dir_a).unwrap();
        let b = ensure_identity(&dir_b).unwrap();
        assert_ne!(a.fingerprint, b.fingerprint);
        std::fs::remove_dir_all(&dir_a).ok();
        std::fs::remove_dir_all(&dir_b).ok();
    }
}
