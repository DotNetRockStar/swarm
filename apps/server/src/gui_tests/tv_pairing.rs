//! Scenario category 3: TV pairing/activation approval. A real handshake
//! needs a real TV on the LAN (that's what `tv_uat_suite.sh` covers against
//! real hardware); at the command level, what's testable in-process is the
//! real error path for an unknown/expired code and the real (empty) state
//! of a freshly started core's peer list — both exercise the actual
//! `ServerCore`/`LanService`/state-DB wiring, not a mock.

use super::harness::test_app_with_media_root;
use crate::{approve_lan_pairing, list_local_peers};
use tauri::Manager;

#[tokio::test]
async fn approve_lan_pairing_rejects_an_unknown_code() {
    let (test_app, _root_dir) = test_app_with_media_root().await;
    let app = test_app.handle();

    let err = approve_lan_pairing(app.clone(), app.state(), "00000000".to_string())
        .await
        .expect_err("a code with no pending TV must be rejected");
    assert!(
        err.to_string().contains("No pending LAN TV"),
        "expected the real invalid-code message, got: {err}"
    );
}

#[tokio::test]
async fn list_local_peers_starts_empty_on_a_fresh_core() {
    let (test_app, _root_dir) = test_app_with_media_root().await;
    let app = test_app.handle();

    let peers = list_local_peers(app.clone(), app.state())
        .await
        .expect("list_local_peers should succeed against a real, empty state DB");
    assert!(peers.is_empty());
}
