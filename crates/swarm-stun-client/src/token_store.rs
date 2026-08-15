//! Encrypted-at-rest storage for a device's STUN access token.
//!
//! Prefers the OS credential store (macOS Keychain / Linux Secret Service /
//! Windows Credential Manager via the `keyring` crate) — the token never
//! touches disk in the clear. Falls back to a permission-restricted file
//! (owner read/write only on Unix) when no keyring backend is available,
//! which real headless Linux deployments (Docker, a bare VPS) hit often; the
//! fallback is logged at `warn` so operators know the weaker guarantee is in
//! effect. Either way the token itself stays the single secret — the STUN
//! server's row is the actual revocation authority, so a leaked file is
//! recoverable by revoking the device.

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum TokenStoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub struct TokenStore {
    /// `None` means file-only mode — the keyring is never touched. Real OS
    /// keyring behavior (backend availability, permission prompts) varies
    /// too much across environments to exercise in automated tests or to
    /// force on headless deployments that are known to have no Secret
    /// Service; both cases construct via [`Self::file_only`].
    entry: Option<keyring::Entry>,
    fallback_path: PathBuf,
}

impl TokenStore {
    /// `service`/`account` identify the credential in the OS store (e.g.
    /// `("swarm-server", "<device fingerprint>")`); `fallback_path` is used
    /// only if the keyring is unavailable.
    pub fn new(service: &str, account: &str, fallback_path: PathBuf) -> Result<Self, TokenStoreError> {
        let entry = keyring::Entry::new(service, account).map_err(|e| {
            // Entry::new itself only fails on malformed service/account
            // strings, not backend availability — that surfaces per-call.
            std::io::Error::other(e.to_string())
        })?;
        Ok(Self { entry: Some(entry), fallback_path })
    }

    /// Skip the OS keyring entirely — always use the permission-restricted
    /// file. For headless deployments with no keyring backend, and for
    /// tests.
    pub fn file_only(fallback_path: PathBuf) -> Self {
        Self { entry: None, fallback_path }
    }

    pub fn save(&self, token: &str) -> Result<(), TokenStoreError> {
        let Some(entry) = &self.entry else { return write_file(&self.fallback_path, token) };
        match entry.set_password(token) {
            Ok(()) => Ok(()),
            Err(err) => {
                tracing::warn!(error = %err, "OS keyring unavailable, storing access token in a restricted file instead");
                write_file(&self.fallback_path, token)
            }
        }
    }

    pub fn load(&self) -> Result<Option<String>, TokenStoreError> {
        let Some(entry) = &self.entry else { return read_file(&self.fallback_path) };
        match entry.get_password() {
            Ok(token) => Ok(Some(token)),
            Err(keyring::Error::NoEntry) => read_file(&self.fallback_path),
            Err(err) => {
                tracing::debug!(error = %err, "OS keyring unavailable, checking fallback file");
                read_file(&self.fallback_path)
            }
        }
    }

    pub fn delete(&self) -> Result<(), TokenStoreError> {
        if let Some(entry) = &self.entry {
            // Best-effort: only the file-removal outcome surfaces below.
            let _ = entry.delete_credential();
        }
        let file_result = if self.fallback_path.exists() { std::fs::remove_file(&self.fallback_path) } else { Ok(()) };
        file_result.map_err(TokenStoreError::from)
    }
}

fn write_file(path: &Path, token: &str) -> Result<(), TokenStoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, token)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn read_file(path: &Path) -> Result<Option<String>, TokenStoreError> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(TokenStoreError::from(err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercises the raw file helpers directly, and separately via the
    /// public `TokenStore::file_only` API below — deliberately never
    /// constructs a keyring-backed `TokenStore` in tests, since that would
    /// touch the real OS keyring (a Keychain permission prompt would hang a
    /// headless test run). The keyring half is reviewed by inspection and
    /// exercised live only inside the shipped desktop app.
    #[test]
    fn file_fallback_roundtrip() {
        let path = std::env::temp_dir().join(format!("swarm-token-test-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        assert_eq!(read_file(&path).unwrap(), None);
        write_file(&path, "secret-token").unwrap();
        assert_eq!(read_file(&path).unwrap(), Some("secret-token".to_string()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn file_only_store_roundtrips_via_the_public_api() {
        let path = std::env::temp_dir().join(format!("swarm-token-store-test-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = TokenStore::file_only(path.clone());
        assert_eq!(store.load().unwrap(), None);
        store.save("secret-token").unwrap();
        assert_eq!(store.load().unwrap(), Some("secret-token".to_string()));
        store.delete().unwrap();
        assert_eq!(store.load().unwrap(), None);
    }
}
