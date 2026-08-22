//! Path-derived catalog entry keys, following Batocera.Drone's rule:
//! `entry_key = sha256(lowercased relative path)[:24]`, and every key is
//! validated as lowercase hex *before* it is allowed anywhere near the
//! filesystem. The key is derived from the library-relative path (never the
//! absolute path) so the same layout yields the same keys on any machine.

use sha2::{Digest, Sha256};

pub const ENTRY_KEY_LEN: usize = 24;

/// Derive the catalog key for a library-relative path. Backslashes are
/// normalized to forward slashes first so Windows and Unix servers agree.
pub fn entry_key(relative_path: &str) -> String {
    let normalized = relative_path.replace('\\', "/").to_lowercase();
    let digest = Sha256::digest(normalized.as_bytes());
    hex::encode(digest)[..ENTRY_KEY_LEN].to_string()
}

/// Keys arriving over the wire must pass this before any lookup. Rejects
/// anything that is not 1-64 chars of lowercase hex (path traversal by
/// construction impossible for valid keys).
pub fn is_valid_entry_key(candidate: &str) -> bool {
    !candidate.is_empty()
        && candidate.len() <= 64
        && candidate
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Vector generated with the original Python derivation.
    #[test]
    fn matches_python_vector() {
        assert_eq!(
            entry_key("movies/Inception (2010)/Inception.mkv"),
            "030fe19c72f2665e6efd018a"
        );
    }

    #[test]
    fn case_and_separator_insensitive() {
        assert_eq!(
            entry_key("Movies\\Inception (2010)\\INCEPTION.MKV"),
            entry_key("movies/inception (2010)/inception.mkv")
        );
    }

    #[test]
    fn validation() {
        assert!(is_valid_entry_key(&entry_key("music/a.flac")));
        assert!(!is_valid_entry_key(""));
        assert!(!is_valid_entry_key("../etc/passwd"));
        assert!(!is_valid_entry_key("030FE19C72F2665E6EFD018A")); // uppercase rejected
        assert!(!is_valid_entry_key(&"a".repeat(65)));
    }
}
