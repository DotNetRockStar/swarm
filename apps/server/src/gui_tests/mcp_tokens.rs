//! Scenario category 6: MCP access-token lifecycle. Settings-only — never
//! touches `AppState::core`, so `test_app()` (no media root) is enough.

use super::harness::test_app;
use crate::{generate_mcp_access_token, get_settings, set_mcp_enabled};

#[tokio::test]
async fn generate_mcp_access_token_persists_a_real_random_token() {
    let test_app = test_app();
    let app = test_app.handle();

    let token = generate_mcp_access_token(app.clone())
        .await
        .expect("generate_mcp_access_token should succeed");
    assert!(token.starts_with("swarm_mcp_"));

    let settings = get_settings(app.clone())
        .await
        .expect("get_settings should succeed");
    assert_eq!(settings.mcp_access_token.as_deref(), Some(token.as_str()));
}

#[tokio::test]
async fn generate_mcp_access_token_rotates_to_a_new_value_each_call() {
    let test_app = test_app();
    let app = test_app.handle();

    let first = generate_mcp_access_token(app.clone())
        .await
        .expect("first generate should succeed");
    let second = generate_mcp_access_token(app.clone())
        .await
        .expect("second generate should succeed");
    assert_ne!(first, second, "each call must mint a fresh token, not reuse one");

    let settings = get_settings(app.clone())
        .await
        .expect("get_settings should succeed");
    assert_eq!(settings.mcp_access_token.as_deref(), Some(second.as_str()));
}

#[tokio::test]
async fn set_mcp_enabled_persists_the_toggle() {
    let test_app = test_app();
    let app = test_app.handle();

    let initial = get_settings(app.clone())
        .await
        .expect("get_settings should succeed");
    assert!(!initial.mcp_enabled, "MCP should default to disabled");

    set_mcp_enabled(app.clone(), true)
        .await
        .expect("set_mcp_enabled should succeed");
    let enabled = get_settings(app.clone())
        .await
        .expect("get_settings should succeed");
    assert!(enabled.mcp_enabled);

    set_mcp_enabled(app.clone(), false)
        .await
        .expect("set_mcp_enabled should succeed");
    let disabled = get_settings(app.clone())
        .await
        .expect("get_settings should succeed");
    assert!(!disabled.mcp_enabled);
}
