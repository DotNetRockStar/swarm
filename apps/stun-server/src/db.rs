//! SQLite persistence. Schema is created idempotently on startup
//! (`CREATE TABLE IF NOT EXISTS` — the Batocera.Drone convention: never bump
//! an applied schema in place, add tables/columns idempotently).
//!
//! Secrets never land in the database in the clear: passwords are Argon2id
//! hashes; session tokens, device access tokens, join codes, and email tokens
//! are stored as SHA-256 hashes of the random value handed out.

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::str::FromStr;

pub async fn connect(database_path: &str) -> sqlx::Result<SqlitePool> {
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{database_path}"))?
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(5))
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new().max_connections(8).connect_with(options).await?;
    init_schema(&pool).await?;
    Ok(pool)
}

async fn init_schema(pool: &SqlitePool) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            email TEXT NOT NULL UNIQUE COLLATE NOCASE,
            password_hash TEXT NOT NULL,
            email_verified_at INTEGER,
            created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS sessions (
            token_hash TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            created_at INTEGER NOT NULL,
            last_seen_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_sessions_expiry ON sessions(expires_at);
        CREATE TABLE IF NOT EXISTS email_tokens (
            token_hash TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            purpose TEXT NOT NULL CHECK (purpose IN ('verify','reset')),
            expires_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS swarms (
            id TEXT PRIMARY KEY,
            owner_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS devices (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            device_type TEXT NOT NULL CHECK (device_type IN ('client','server','both')),
            machine_id TEXT NOT NULL,
            cert_fingerprint TEXT NOT NULL,
            platform TEXT NOT NULL,
            app_version TEXT NOT NULL,
            access_token_hash TEXT NOT NULL,
            revoked_at INTEGER,
            last_seen_at INTEGER,
            created_at INTEGER NOT NULL,
            UNIQUE(user_id, machine_id)
        );
        CREATE TABLE IF NOT EXISTS device_metadata (
            device_id TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
            key TEXT NOT NULL,
            value TEXT NOT NULL,
            PRIMARY KEY (device_id, key)
        );
        CREATE TABLE IF NOT EXISTS swarm_devices (
            swarm_id TEXT NOT NULL REFERENCES swarms(id) ON DELETE CASCADE,
            device_id TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
            joined_at INTEGER NOT NULL,
            PRIMARY KEY (swarm_id, device_id)
        );
        CREATE TABLE IF NOT EXISTS join_codes (
            id TEXT PRIMARY KEY,
            swarm_id TEXT NOT NULL REFERENCES swarms(id) ON DELETE CASCADE,
            created_by TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            code_hash TEXT NOT NULL,
            device_type_hint TEXT,
            expires_at INTEGER NOT NULL,
            redeemed_by_device TEXT,
            created_at INTEGER NOT NULL
        );
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Unix seconds → RFC 3339 UTC, for API responses.
pub fn rfc3339(secs: i64) -> String {
    // Days-based civil-from-epoch conversion (Howard Hinnant's algorithm);
    // avoids pulling a chrono dependency for one formatting need.
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (h, m, s) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_formats_known_instants() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_755_216_000), "2025-08-15T00:00:00Z");
    }
}
