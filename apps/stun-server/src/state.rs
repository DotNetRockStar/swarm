//! Shared application state handed to every route.

use crate::config::Config;
use crate::email::Mailer;
use crate::hub::Hub;
use crate::security::{AllocationLimiter, BruteForceBlocker};
use sqlx::SqlitePool;
use std::sync::Arc;

pub struct AppState {
    pub db: SqlitePool,
    pub hub: Hub,
    pub config: Config,
    pub blocker: BruteForceBlocker,
    pub activation_allocations: AllocationLimiter,
    pub managed_swarm_allocations: AllocationLimiter,
    pub mailer: Mailer,
}

pub type SharedState = Arc<AppState>;
