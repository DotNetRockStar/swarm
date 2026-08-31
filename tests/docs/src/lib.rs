//! Guardrails for `docs/guide/` — the interactive, audience-switching System
//! Guide added for issue #130.
//!
//! The guide is a single self-contained HTML file that restates facts owned by
//! `README.md`, `docs/PROTOCOL.md`, the client/deploy READMEs, and
//! `scripts/tests/TV_TESTING.md`. Prose drifts silently; these tests fail loudly
//! when it does. See `tests/guide.rs`.
//!
//! Deliberately dependency-free (std only) so it costs nothing on
//! `cargo test --workspace`.

/// Repo root, resolved from this crate's manifest dir (`<root>/tests/docs`).
pub fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("resolve repo root")
}

/// Read a repo-relative file to a string.
pub fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Every `href="#anchor"` referenced in `html`.
pub fn anchor_refs(html: &str) -> Vec<String> {
    collect_after(html, "href=\"#")
}

/// Every `id="value"` defined in `html`.
pub fn ids(html: &str) -> Vec<String> {
    collect_after(html, "id=\"")
}

fn collect_after(html: &str, marker: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find(marker) {
        rest = &rest[i + marker.len()..];
        if let Some(end) = rest.find('"') {
            out.push(rest[..end].to_string());
        }
    }
    out
}
