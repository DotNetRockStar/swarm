//! `sample-fp-v1` content fingerprinting, ported byte-for-byte from
//! Batocera.Drone's `app/common/fingerprint.py`.
//!
//! MD5 over the file size (8 bytes little-endian) followed by either the whole
//! file (small files, exact) or three fixed 64 KiB windows (head, middle,
//! tail). Constant cost regardless of file size — multi-GB media files are
//! never read end to end. Folding the size into the digest means two files of
//! different size can never collide. Not a cryptographic hash; for "is this
//! the same file on another device?" the collision probability is negligible,
//! and cross-implementation determinism is what makes multi-server catalog
//! merging work.

use md5::{Digest, Md5};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub const FINGERPRINT_ALGORITHM: &str = "sample-fp-v1";
pub const SAMPLE_BYTES: u64 = 64 * 1024;
/// Files at or below this size are hashed whole (exact). Must stay >= 3x the
/// sample size so the three windows never overlap.
pub const SMALL_FILE_BYTES: u64 = 3 * SAMPLE_BYTES;

/// Fingerprint a file on disk.
pub fn fingerprint_file(path: &Path) -> std::io::Result<String> {
    let file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    fingerprint_reader(size, file)
}

/// Fingerprint any seekable byte source of known size.
pub fn fingerprint_reader<R: Read + Seek>(size: u64, mut reader: R) -> std::io::Result<String> {
    let mut digest = Md5::new();
    digest.update(size.to_le_bytes());
    if size <= SMALL_FILE_BYTES {
        let mut buf = Vec::with_capacity(size as usize);
        reader.read_to_end(&mut buf)?;
        digest.update(&buf);
    } else {
        let mut window = vec![0u8; SAMPLE_BYTES as usize];
        reader.read_exact(&mut window)?;
        digest.update(&window);
        let middle = (size / 2).saturating_sub(SAMPLE_BYTES / 2);
        reader.seek(SeekFrom::Start(middle))?;
        reader.read_exact(&mut window)?;
        digest.update(&window);
        reader.seek(SeekFrom::Start(size - SAMPLE_BYTES))?;
        reader.read_exact(&mut window)?;
        digest.update(&window);
    }
    Ok(hex::encode(digest.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn fp(data: &[u8]) -> String {
        fingerprint_reader(data.len() as u64, Cursor::new(data)).unwrap()
    }

    // Vectors generated with the original Python implementation
    // (batocera.drone app/common/fingerprint.py) — these pin byte
    // compatibility across the two codebases.
    #[test]
    fn matches_python_small_file_vector() {
        assert_eq!(
            fp(b"swarm test vector\n"),
            "704ac5a4284267953aab77855e0e32aa"
        );
    }

    #[test]
    fn matches_python_large_file_vector() {
        let large: Vec<u8> = (0..300_000usize).map(|i| (i % 251) as u8).collect();
        assert_eq!(fp(&large), "4dd20b8f2a90b824295b147ac6b65bd9");
    }

    #[test]
    fn matches_python_empty_file_vector() {
        assert_eq!(fp(b""), "7dea362b3fac8e00956a4952a3d4f474");
    }

    #[test]
    fn size_is_folded_into_digest() {
        // Same leading bytes, different lengths — must differ even though the
        // small-file path hashes both whole.
        assert_ne!(fp(b"abc"), fp(b"abc\0"));
    }

    #[test]
    fn threshold_boundary_uses_whole_file() {
        let at = vec![7u8; SMALL_FILE_BYTES as usize];
        let over = vec![7u8; SMALL_FILE_BYTES as usize + 1];
        // Both succeed; the boundary file takes the exact path, one byte more
        // takes the windowed path, and they must not collide.
        assert_ne!(fp(&at), fp(&over));
    }

    #[test]
    fn windowed_files_differing_only_mid_body_differ() {
        let mut a = vec![0u8; 1_000_000];
        let mut b = a.clone();
        a[500_000] = 1; // inside the middle window
        b[500_000] = 2;
        assert_ne!(fp(&a), fp(&b));
    }

    #[test]
    fn file_roundtrip_matches_reader() {
        let dir = std::env::temp_dir().join("swarm-fp-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("vector.bin");
        std::fs::write(&path, b"swarm test vector\n").unwrap();
        assert_eq!(
            fingerprint_file(&path).unwrap(),
            "704ac5a4284267953aab77855e0e32aa"
        );
        std::fs::remove_file(&path).ok();
    }
}
