//! Multiple named library roots — lets a server point at more than one
//! filesystem location (e.g. a local drive plus a mounted NAS share) while
//! keeping `entry_key` (which hashes `relative_path` alone, see
//! `swarm_core::entry_key`) collision-free across roots: two roots
//! containing the same sub-path would otherwise produce the same key.
//!
//! Single-root installs — the overwhelmingly common case — get
//! byte-identical `relative_path`/`entry_key` values whether this module
//! exists or not: no `{label}/` prefix is ever applied unless 2+ roots are
//! configured. This is a deliberate, permanent asymmetry rather than a
//! migration shim. Multi-root support has never shipped, so there is no
//! installed base whose paths need to stay stable across a 1→2 transition —
//! nothing is gained by forcing a prefix onto the single-root case just to
//! make a future transition uniform.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaRoot {
    pub label: String,
    pub path: PathBuf,
}

/// Resolves a stored `relative_path` (as written by `scan::scan_roots`) back
/// to an absolute filesystem path, and vice versa. Single source of truth
/// for the "1 root → no prefix, 2+ roots → `{label}/` prefix" convention, so
/// scanning, serving, and artwork-writing can't drift out of sync on it.
#[derive(Debug, Clone)]
pub struct RootResolver {
    roots: Vec<MediaRoot>,
}

impl RootResolver {
    /// # Panics
    /// If `roots` is empty — at least one root is always required.
    pub fn new(roots: Vec<MediaRoot>) -> Self {
        assert!(!roots.is_empty(), "RootResolver requires at least one media root");
        Self { roots }
    }

    pub fn single(path: PathBuf) -> Self {
        Self { roots: vec![MediaRoot { label: "local".to_string(), path }] }
    }

    pub fn roots(&self) -> &[MediaRoot] {
        &self.roots
    }

    fn multi(&self) -> bool {
        self.roots.len() > 1
    }

    /// Absolute filesystem path for a stored `relative_path`.
    pub fn resolve(&self, relative_path: &str) -> PathBuf {
        let (root, rest) = self.split(relative_path);
        root.join(rest)
    }

    /// (absolute root directory, path under that root) for a stored
    /// `relative_path`. Falls back to the first configured root when the
    /// path carries no recognized `{label}/` prefix (always true in the
    /// single-root case, and a safe degrade for an unrecognized label —
    /// callers resolving a filesystem path from the result simply fail to
    /// find the file rather than panicking).
    pub fn split(&self, relative_path: &str) -> (PathBuf, String) {
        if self.multi() {
            if let Some((label, rest)) = relative_path.split_once('/') {
                if let Some(root) = self.roots.iter().find(|r| r.label == label) {
                    return (root.path.clone(), rest.to_string());
                }
            }
        }
        let root = self.roots.first().map(|r| r.path.clone()).unwrap_or_default();
        (root, relative_path.to_string())
    }

    /// The label that owns a stored `relative_path` — used to round-trip
    /// [`Self::compose`] after resolving a path back out with [`Self::split`]
    /// (e.g. writing a new artwork file alongside an already-scanned entry).
    pub fn label_for(&self, relative_path: &str) -> String {
        if self.multi() {
            if let Some((label, _)) = relative_path.split_once('/') {
                if self.roots.iter().any(|r| r.label == label) {
                    return label.to_string();
                }
            }
        }
        self.roots.first().map(|r| r.label.clone()).unwrap_or_default()
    }

    /// Build a stored `relative_path` from a root's label and a path under
    /// that root — the inverse of [`Self::split`].
    pub fn compose(&self, label: &str, path_under_root: &str) -> String {
        if self.multi() {
            format!("{label}/{path_under_root}")
        } else {
            path_under_root.to_string()
        }
    }
}

/// Parse `SWARM_MEDIA_ROOTS`'s `label=path,label2=path2` format.
pub fn parse_roots_env(value: &str) -> Vec<MediaRoot> {
    value
        .split(',')
        .filter_map(|entry| {
            let entry = entry.trim();
            if entry.is_empty() {
                return None;
            }
            let (label, path) = entry.split_once('=')?;
            let label = label.trim();
            let path = path.trim();
            if label.is_empty() || path.is_empty() {
                return None;
            }
            Some(MediaRoot { label: label.to_string(), path: PathBuf::from(path) })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_roots() -> RootResolver {
        RootResolver::new(vec![
            MediaRoot { label: "local".into(), path: PathBuf::from("/media") },
            MediaRoot { label: "nas".into(), path: PathBuf::from("/Volumes/nas") },
        ])
    }

    #[test]
    fn single_root_applies_no_prefix() {
        let r = RootResolver::single(PathBuf::from("/media"));
        assert_eq!(r.resolve("movies/Foo.mkv"), PathBuf::from("/media/movies/Foo.mkv"));
        assert_eq!(r.compose("local", "movies/Foo.mkv"), "movies/Foo.mkv");
        assert_eq!(r.label_for("movies/Foo.mkv"), "local");
    }

    #[test]
    fn multi_root_resolves_by_label_prefix() {
        let r = two_roots();
        assert_eq!(r.resolve("nas/movies/Foo.mkv"), PathBuf::from("/Volumes/nas/movies/Foo.mkv"));
        assert_eq!(r.resolve("local/movies/Foo.mkv"), PathBuf::from("/media/movies/Foo.mkv"));
        assert_eq!(r.label_for("nas/movies/Foo.mkv"), "nas");
    }

    #[test]
    fn multi_root_compose_round_trips_through_split() {
        let r = two_roots();
        let stored = r.compose("nas", "movies/Foo.mkv");
        assert_eq!(stored, "nas/movies/Foo.mkv");
        let (root, rest) = r.split(&stored);
        assert_eq!(root, PathBuf::from("/Volumes/nas"));
        assert_eq!(rest, "movies/Foo.mkv");
    }

    #[test]
    fn parses_label_equals_path_pairs() {
        let roots = parse_roots_env("local=/media,nas=/Volumes/nas");
        assert_eq!(
            roots,
            vec![
                MediaRoot { label: "local".into(), path: PathBuf::from("/media") },
                MediaRoot { label: "nas".into(), path: PathBuf::from("/Volumes/nas") },
            ]
        );
    }

    #[test]
    fn parse_roots_env_skips_malformed_entries() {
        assert_eq!(parse_roots_env(""), vec![]);
        assert_eq!(parse_roots_env("no-equals-sign"), vec![]);
        assert_eq!(parse_roots_env("=novalue,label=,ok=/path"), vec![MediaRoot { label: "ok".into(), path: PathBuf::from("/path") }]);
    }
}
