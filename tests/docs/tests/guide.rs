//! Structural + factual guardrails for `docs/guide/index.html` (issue #130).
//!
//! These are unit-style checks (no server, no hardware). Two jobs:
//!   1. The interactive shell keeps working — audience switch, self-contained
//!      assets, no broken in-page links.
//!   2. The prose stays honest — key facts still match their source of truth in
//!      `README.md` / `docs/PROTOCOL.md` / `scripts/tests/TV_TESTING.md`.

use swarm_docs::{anchor_refs, ids, read, repo_root};

const GUIDE: &str = "docs/guide/index.html";

fn guide() -> String {
    read(GUIDE)
}

#[test]
fn guide_file_is_present_and_substantial() {
    let html = guide();
    assert!(
        html.len() > 20_000,
        "guide shrank unexpectedly ({} bytes) — is it still comprehensive?",
        html.len()
    );
    assert!(html.starts_with("<!DOCTYPE html>"));
    assert!(html.contains("<title>SWARM — System Guide</title>"));
}

#[test]
fn audience_switch_is_wired() {
    let html = guide();
    // The two audiences the issue asks for.
    for needle in [
        r#"data-set-audience="user""#,
        r#"data-set-audience="engineer""#,
        r#"data-audience="engineer""#,
        r#"data-audience="user""#,
        // Persisted choice.
        r#""swarm-docs-audience""#,
        // Default state + the CSS that actually does the hiding.
        r#"<body class="audience-user">"#,
        "body.audience-user [data-audience=\"engineer\"]",
        "body.audience-engineer [data-audience=\"user\"]",
        // Graceful no-JS degradation.
        "<noscript>",
    ] {
        assert!(html.contains(needle), "guide is missing: {needle}");
    }
}

#[test]
fn guide_is_self_contained_no_cdn() {
    let html = guide();
    assert!(
        !html.contains("<script src="),
        "guide must not load external scripts — keep it self-contained"
    );
    assert!(
        !html.contains(r#"rel="stylesheet""#),
        "guide must not link external stylesheets — keep CSS inline"
    );
}

#[test]
fn every_in_page_link_resolves() {
    let html = guide();
    let defined: std::collections::HashSet<String> = ids(&html).into_iter().collect();
    let mut missing: Vec<String> = anchor_refs(&html)
        .into_iter()
        .filter(|a| !a.is_empty() && !defined.contains(a))
        .collect();
    missing.sort();
    missing.dedup();
    assert!(missing.is_empty(), "dangling in-page anchors: {missing:?}");
}

#[test]
fn required_sections_all_exist() {
    let html = guide();
    let defined: std::collections::HashSet<String> = ids(&html).into_iter().collect();
    for id in [
        "overview",
        "audiences",
        "glossary",
        "devices",
        "media-server",
        "tv-clients",
        "stun-server",
        "lan",
        "internet",
        "media",
        "security",
        "protocol",
        "technology",
        "operations",
        "testing",
        "testing-unit",
        "testing-uat",
        "testing-integration",
        "roadmap",
    ] {
        assert!(defined.contains(id), "guide lost required section: #{id}");
    }
}

#[test]
fn covers_every_area_the_issue_names() {
    let html = guide().to_lowercase();
    // Issue #130: "from the devices, to the media server, STUN server, LAN
    // connections, and especially the fine detail to security ... a section
    // about the level of testing (unit, UAT, and integration) ... diagrams,
    // flows, security, and technology choices."
    for topic in [
        "media server",
        "stun",
        "lan",
        "security",
        "unit",
        "uat",
        "integration",
        "technology choices",
        "fire tv",
        "roku",
        "hole punch",
    ] {
        assert!(html.contains(topic), "guide never mentions: {topic}");
    }
    assert!(html.contains("<svg"), "guide should include diagrams (inline SVG)");
    assert!(
        html.matches("<svg").count() >= 4,
        "expected several diagrams, found {}",
        guide().matches("<svg").count()
    );
    assert!(
        html.contains("class=\"flow\""),
        "guide should include step-by-step flows"
    );
}

#[test]
fn security_facts_match_protocol_spec() {
    let html = guide();
    for fact in [
        "Argon2id",
        "access_token",
        "TOFU",
        "self-signed",
        "fingerprint",
        "pinned",
        "TLS 1.3",
        "QUIC",
        "no relay",
        "PROTOCOL_VERSION",
        "deny_unknown_fields",
        "is_valid_entry_key",
        "reject_cross_site",
        "require_lan",
        "Sec-Fetch-Site",
    ] {
        assert!(html.contains(fact), "security section dropped: {fact}");
    }
}

#[test]
fn env_var_overrides_stay_in_sync_with_readme() {
    let html = guide();
    let readme = read("README.md");
    for var in [
        "SWARM_PEER_BIND",
        "SWARM_RENDEZVOUS_URL",
        "SWARM_MAX_UPLOAD_MBPS",
        "SWARM_UPLOAD_RESERVE_PERCENT",
        "SWARM_MAX_STREAMS",
        "SWARM_FFMPEG_PATH",
        "SWARM_TRANSCODING_DISABLED",
    ] {
        assert!(readme.contains(var), "README no longer documents {var} — update the guide");
        assert!(html.contains(var), "guide is missing env override {var}");
    }
}

#[test]
fn hls_ladder_matches_protocol_spec() {
    let html = guide();
    let protocol = read("docs/PROTOCOL.md");
    for rung in ["1920×1080", "1280×720", "854×480", "640×360"] {
        assert!(protocol.contains(rung), "PROTOCOL.md ladder changed ({rung}) — resync the guide");
        assert!(html.contains(rung), "guide HLS ladder is missing {rung}");
    }
    // Punch magic string is a wire constant; keep the guide's copy exact.
    assert!(protocol.contains(r#"b"swarm-punch-v1""#));
    assert!(html.contains(r#"b"swarm-punch-v1""#), "guide punch magic drifted from spec");
}

#[test]
fn paths_the_guide_points_at_still_exist_on_disk() {
    let root = repo_root();
    for rel in [
        "docs/PROTOCOL.md",
        "docs/reference",
        "scripts/tests/TV_TESTING.md",
        "scripts/tests/media_server_uat_tests.sh",
        "scripts/tests/tv_uat_suite.sh",
        "scripts/tests/tv_e2e_suite.sh",
        "scripts/tests/tv_uat_resilience_suite.sh",
        "scripts/tests/full_uat_suite.sh",
        "scripts/tests/full_uat_cron.sh",
        "tests/integration",
        "apps/server/src/gui_tests",
        "apps/server/tests/lan_direct_play.rs",
        "apps/server/src/http_media.rs",
    ] {
        assert!(
            root.join(rel).exists(),
            "guide points at {rel}, which no longer exists"
        );
    }
}

#[test]
fn guide_names_the_test_layers_it_documents() {
    let html = guide();
    for needle in [
        "cargo test --workspace",
        ":core:test",
        "media_server_uat_tests.sh",
        "tv_uat_suite.sh",
        "tv_e2e_suite.sh",
        "tv_uat_resilience_suite.sh",
        "full_uat_suite.sh",
        "full_uat_cron.sh",
        "gui_tests",
        "lan_direct_play.rs",
        "http_media.rs",
        "tests/integration/",
        "swarm-e2e-suite-lockdown",
    ] {
        assert!(html.contains(needle), "testing section dropped: {needle}");
    }
}

#[test]
fn relative_links_out_of_the_guide_resolve() {
    let guide_dir = repo_root().join("docs/guide");
    for rel in ["../PROTOCOL.md", "../reference/", "../../scripts/tests/TV_TESTING.md"] {
        assert!(
            guide().contains(&format!("\"{rel}\"")),
            "guide should link to {rel}"
        );
        assert!(
            guide_dir.join(rel).exists(),
            "relative link {rel} from docs/guide/ is broken"
        );
    }
}
