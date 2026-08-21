//! Passwords, opaque tokens, join codes, and the per-IP brute-force blocker.
//!
//! Ported patterns from Batocera.Drone `app/common/auth.py` /
//! `local_network.py`: opaque DB-backed tokens (row = revocable source of
//! truth, deliberately not JWT), 8-digit crypto-random single-use codes with
//! constant-time comparison, and a 5-strikes-in-60s → 5-minute-block limiter.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default()
        .hash_password(password.as_bytes(), &salt)?
        .to_string())
}

pub fn verify_password(password: &str, stored_hash: &str) -> bool {
    PasswordHash::new(stored_hash)
        .map(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
        .unwrap_or(false)
}

/// Small denylist backstop on top of the length rule; a full breached-password
/// list can replace this later without an API change.
const COMMON_PASSWORDS: &[&str] = &[
    "password12",
    "password123",
    "qwertyuiop",
    "1234567890",
    "letmeincool",
    "iloveyou12",
    "adminadmin",
    "welcome123",
    "monkey12345",
    "dragon12345",
];

pub fn validate_password(password: &str) -> Result<(), &'static str> {
    if password.len() < 10 {
        return Err("password must be at least 10 characters");
    }
    if password.len() > 512 {
        return Err("password too long");
    }
    if COMMON_PASSWORDS.contains(&password.to_lowercase().as_str()) {
        return Err("that password is too common");
    }
    Ok(())
}

/// 256-bit opaque token as lowercase hex. The caller hands the value out once
/// and stores only [`token_hash`].
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// 8-digit join code, uniform over 00000000..=99999999.
pub fn generate_join_code() -> String {
    // Rejection-sample to keep the distribution uniform.
    loop {
        let n = OsRng.next_u32();
        if n < 4_200_000_000 {
            return format!("{:08}", n % 100_000_000);
        }
    }
}

pub fn new_id() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// In-memory per-IP failure tracker: 5 auth failures within 60s block the IP
/// for 5 minutes. Single-instance state by design (one process per deploy);
/// loopback is exempt so local tooling can't lock itself out.
pub struct BruteForceBlocker {
    state: Mutex<HashMap<IpAddr, Entry>>,
}

/// Fixed-window successful-allocation limiter. Authentication failure limits
/// do not stop a valid-looking anonymous caller from filling storage, so
/// managed-swarm provisioning and TV activation creation use this separate
/// per-IP budget. Loopback remains exempt for local development and tests.
pub struct AllocationLimiter {
    state: Mutex<HashMap<IpAddr, Vec<Instant>>>,
    max: usize,
    window: Duration,
}

impl AllocationLimiter {
    pub fn new(max: usize, window: Duration) -> Self {
        Self {
            state: Mutex::new(HashMap::new()),
            max,
            window,
        }
    }

    pub fn allow(&self, ip: IpAddr) -> bool {
        if ip.is_loopback() {
            return true;
        }
        let now = Instant::now();
        let mut state = self.state.lock().unwrap();
        let entries = state.entry(ip).or_default();
        entries.retain(|at| now.duration_since(*at) < self.window);
        if entries.len() >= self.max {
            return false;
        }
        entries.push(now);
        true
    }
}

struct Entry {
    failures: Vec<Instant>,
    blocked_until: Option<Instant>,
}

const WINDOW: Duration = Duration::from_secs(60);
const BLOCK: Duration = Duration::from_secs(300);
const MAX_FAILURES: usize = 5;

impl Default for BruteForceBlocker {
    fn default() -> Self {
        Self::new()
    }
}

impl BruteForceBlocker {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(HashMap::new()),
        }
    }

    pub fn is_blocked(&self, ip: IpAddr) -> bool {
        if ip.is_loopback() {
            return false;
        }
        let mut state = self.state.lock().unwrap();
        match state.get_mut(&ip) {
            Some(entry) => match entry.blocked_until {
                Some(until) if Instant::now() < until => true,
                Some(_) => {
                    state.remove(&ip);
                    false
                }
                None => false,
            },
            None => false,
        }
    }

    pub fn record_failure(&self, ip: IpAddr) {
        if ip.is_loopback() {
            return;
        }
        let now = Instant::now();
        let mut state = self.state.lock().unwrap();
        let entry = state.entry(ip).or_insert(Entry {
            failures: Vec::new(),
            blocked_until: None,
        });
        entry.failures.retain(|t| now.duration_since(*t) < WINDOW);
        entry.failures.push(now);
        if entry.failures.len() >= MAX_FAILURES {
            entry.blocked_until = Some(now + BLOCK);
            entry.failures.clear();
        }
    }

    pub fn record_success(&self, ip: IpAddr) {
        self.state.lock().unwrap().remove(&ip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hash_roundtrip() {
        let hash = hash_password("correct horse battery").unwrap();
        assert!(verify_password("correct horse battery", &hash));
        assert!(!verify_password("wrong", &hash));
    }

    #[test]
    fn password_rules() {
        assert!(validate_password("short").is_err());
        assert!(validate_password("correct horse battery").is_ok());
        assert!(validate_password("password123").is_err()); // common list
        assert!(validate_password("Password123").is_err()); // case-folded common check
    }

    #[test]
    fn join_code_shape() {
        for _ in 0..100 {
            let code = generate_join_code();
            assert_eq!(code.len(), 8);
            assert!(code.bytes().all(|b| b.is_ascii_digit()));
        }
    }

    #[test]
    fn blocker_blocks_after_five_failures() {
        let blocker = BruteForceBlocker::new();
        let ip: IpAddr = "203.0.113.7".parse().unwrap();
        for _ in 0..4 {
            blocker.record_failure(ip);
            assert!(!blocker.is_blocked(ip));
        }
        blocker.record_failure(ip);
        assert!(blocker.is_blocked(ip));
    }

    #[test]
    fn loopback_exempt() {
        let blocker = BruteForceBlocker::new();
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        for _ in 0..10 {
            blocker.record_failure(ip);
        }
        assert!(!blocker.is_blocked(ip));
    }

    #[test]
    fn allocation_limiter_enforces_success_budget() {
        let limiter = AllocationLimiter::new(2, Duration::from_secs(60));
        let ip: IpAddr = "203.0.113.9".parse().unwrap();
        assert!(limiter.allow(ip));
        assert!(limiter.allow(ip));
        assert!(!limiter.allow(ip));
    }
}
