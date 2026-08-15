//! Shared application state handed to every route.

use crate::config::Config;
use crate::hub::Hub;
use crate::security::BruteForceBlocker;
use sqlx::SqlitePool;
use std::sync::Arc;

pub struct AppState {
    pub db: SqlitePool,
    pub hub: Hub,
    pub config: Config,
    pub blocker: BruteForceBlocker,
}

pub type SharedState = Arc<AppState>;
