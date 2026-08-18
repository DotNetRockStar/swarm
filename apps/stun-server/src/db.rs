//! SQLite persistence. Schema is created and upgraded idempotently on startup.
//! Additive changes use `IF NOT EXISTS`; the one legacy table rebuild below
//! upgrades databases created before `join_codes.redeemed_by_device` became a
//! real foreign key.
//!
//! Secrets never land in the database in the clear: passwords are Argon2id
//! hashes; session tokens, device access tokens, join codes, and email tokens
//! are stored as SHA-256 hashes of the random value handed out.

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;
use std::str::FromStr;

pub async fn connect(database_path: &str) -> sqlx::Result<SqlitePool> {
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{database_path}"))?
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        // Favor durability for account and device credentials. Write volume
        // is low enough that WAL + FULL synchronization is the right trade.
        .synchronous(SqliteSynchronous::Full)
        .busy_timeout(std::time::Duration::from_secs(5))
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await?;
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
            device_type_hint TEXT CHECK (
                device_type_hint IS NULL OR device_type_hint IN ('client','server','both')
            ),
            expires_at INTEGER NOT NULL,
            redeemed_by_device TEXT REFERENCES devices(id) ON DELETE CASCADE,
            created_at INTEGER NOT NULL
        );
        "#,
    )
    .execute(pool)
    .await?;

    migrate_join_code_device_relation(pool).await?;

    // SQLite does not automatically index foreign-key child columns. Besides
    // accelerating the application's hot lookups, these indexes keep parent
    // deletes from scanning whole child tables while enforcing cascades.
    sqlx::query(
        r#"
        CREATE INDEX IF NOT EXISTS idx_sessions_expiry
            ON sessions(expires_at);
        CREATE INDEX IF NOT EXISTS idx_sessions_user
            ON sessions(user_id);
        CREATE INDEX IF NOT EXISTS idx_email_tokens_user_purpose
            ON email_tokens(user_id, purpose);
        CREATE INDEX IF NOT EXISTS idx_swarms_owner_created
            ON swarms(owner_user_id, created_at);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_devices_access_token
            ON devices(access_token_hash);
        CREATE INDEX IF NOT EXISTS idx_devices_active_user_created
            ON devices(user_id, created_at) WHERE revoked_at IS NULL;
        CREATE INDEX IF NOT EXISTS idx_swarm_devices_device
            ON swarm_devices(device_id, swarm_id);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_join_codes_unredeemed_code
            ON join_codes(code_hash) WHERE redeemed_by_device IS NULL;
        CREATE INDEX IF NOT EXISTS idx_join_codes_expiry
            ON join_codes(expires_at);
        CREATE INDEX IF NOT EXISTS idx_join_codes_swarm
            ON join_codes(swarm_id);
        CREATE INDEX IF NOT EXISTS idx_join_codes_creator
            ON join_codes(created_by);
        CREATE INDEX IF NOT EXISTS idx_join_codes_redeemed_device
            ON join_codes(redeemed_by_device) WHERE redeemed_by_device IS NOT NULL;
        "#,
    )
    .execute(pool)
    .await?;

    let violations: Vec<(String, Option<i64>, String, i64)> =
        sqlx::query_as("PRAGMA foreign_key_check")
            .fetch_all(pool)
            .await?;
    if !violations.is_empty() {
        return Err(sqlx::Error::Protocol(format!(
            "database contains {} foreign-key violation(s); first violation: {:?}",
            violations.len(),
            violations[0]
        )));
    }
    sqlx::query("PRAGMA optimize").execute(pool).await?;
    Ok(())
}

/// SQLite cannot add a foreign key to an existing column. Detect the legacy
/// `join_codes` layout and rebuild it transactionally, preserving valid data.
async fn migrate_join_code_device_relation(pool: &SqlitePool) -> sqlx::Result<()> {
    type ForeignKeyRow = (i64, i64, String, String, String, String, String, String);
    let foreign_keys: Vec<ForeignKeyRow> = sqlx::query_as("PRAGMA foreign_key_list('join_codes')")
        .fetch_all(pool)
        .await?;
    if foreign_keys.iter().any(|(_, _, table, from, to, _, _, _)| {
        table == "devices" && from == "redeemed_by_device" && to == "id"
    }) {
        return Ok(());
    }

    let mut tx = pool.begin().await?;
    sqlx::query("DROP TABLE IF EXISTS join_codes_rebuild")
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        r#"
        CREATE TABLE join_codes_rebuild (
            id TEXT PRIMARY KEY,
            swarm_id TEXT NOT NULL REFERENCES swarms(id) ON DELETE CASCADE,
            created_by TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            code_hash TEXT NOT NULL,
            device_type_hint TEXT CHECK (
                device_type_hint IS NULL OR device_type_hint IN ('client','server','both')
            ),
            expires_at INTEGER NOT NULL,
            redeemed_by_device TEXT REFERENCES devices(id) ON DELETE CASCADE,
            created_at INTEGER NOT NULL
        )
        "#,
    )
    .execute(&mut *tx)
    .await?;
    // A pre-constraint database could theoretically contain an orphaned
    // redeemed-device value. Treat that code as consumed and discard it
    // rather than making it redeemable again by nulling the value.
    sqlx::query(
        r#"
        INSERT INTO join_codes_rebuild
            (id, swarm_id, created_by, code_hash, device_type_hint, expires_at,
             redeemed_by_device, created_at)
        SELECT jc.id, jc.swarm_id, jc.created_by, jc.code_hash,
               jc.device_type_hint, jc.expires_at, jc.redeemed_by_device,
               jc.created_at
        FROM join_codes jc
        WHERE jc.redeemed_by_device IS NULL
           OR EXISTS (SELECT 1 FROM devices d WHERE d.id = jc.redeemed_by_device)
        "#,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query("DROP TABLE join_codes")
        .execute(&mut *tx)
        .await?;
    sqlx::query("ALTER TABLE join_codes_rebuild RENAME TO join_codes")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
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

    fn test_database_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "swarm-{label}-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    async fn remove_test_database(path: &std::path::Path, pool: SqlitePool) {
        pool.close().await;
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn rfc3339_formats_known_instants() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_755_216_000), "2025-08-15T00:00:00Z");
    }

    #[tokio::test]
    async fn schema_enforces_relations_uniqueness_and_cascades() {
        let path = test_database_path("schema");
        let pool = connect(path.to_str().unwrap()).await.unwrap();

        let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(foreign_keys, 1);
        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(journal_mode, "wal");

        let invalid_child = sqlx::query(
            "INSERT INTO sessions (token_hash, user_id, created_at, last_seen_at, expires_at) VALUES ('s', 'missing', 1, 1, 2)",
        )
        .execute(&pool)
        .await;
        assert!(
            invalid_child.is_err(),
            "sessions.user_id must be a real user"
        );

        sqlx::query("INSERT INTO users (id, email, password_hash, created_at) VALUES ('u', 'u@example.com', 'hash', 1)")
            .execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO swarms (id, owner_user_id, name, created_at) VALUES ('w', 'u', 'Home', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO devices (id, user_id, name, device_type, machine_id, cert_fingerprint, platform, app_version, access_token_hash, created_at) VALUES ('d', 'u', 'TV', 'client', 'm', 'fp', 'test', '1', 'access', 1)",
        )
        .execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO join_codes (id, swarm_id, created_by, code_hash, expires_at, redeemed_by_device, created_at) VALUES ('j', 'w', 'u', 'code', 2, 'd', 1)",
        )
        .execute(&pool).await.unwrap();

        let duplicate_token = sqlx::query(
            "INSERT INTO devices (id, user_id, name, device_type, machine_id, cert_fingerprint, platform, app_version, access_token_hash, created_at) VALUES ('d2', 'u', 'TV 2', 'client', 'm2', 'fp2', 'test', '1', 'access', 1)",
        )
        .execute(&pool)
        .await;
        assert!(
            duplicate_token.is_err(),
            "device access-token hashes must be unique"
        );

        sqlx::query("DELETE FROM devices WHERE id = 'd'")
            .execute(&pool)
            .await
            .unwrap();
        let remaining_code: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM join_codes WHERE id = 'j'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            remaining_code, 0,
            "redeemed join codes must follow their device cascade"
        );

        remove_test_database(&path, pool).await;
    }

    #[tokio::test]
    async fn legacy_join_codes_table_is_upgraded() {
        let path = test_database_path("migration");
        let url = format!("sqlite://{}", path.display());
        let options = SqliteConnectOptions::from_str(&url)
            .unwrap()
            .create_if_missing(true)
            .foreign_keys(true);
        let legacy = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE join_codes (
                id TEXT PRIMARY KEY,
                swarm_id TEXT NOT NULL,
                created_by TEXT NOT NULL,
                code_hash TEXT NOT NULL,
                device_type_hint TEXT,
                expires_at INTEGER NOT NULL,
                redeemed_by_device TEXT,
                created_at INTEGER NOT NULL
            )
            "#,
        )
        .execute(&legacy)
        .await
        .unwrap();
        legacy.close().await;

        let pool = connect(path.to_str().unwrap()).await.unwrap();
        type ForeignKeyRow = (i64, i64, String, String, String, String, String, String);
        let foreign_keys: Vec<ForeignKeyRow> =
            sqlx::query_as("PRAGMA foreign_key_list('join_codes')")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(foreign_keys
            .iter()
            .any(|(_, _, table, from, to, _, on_delete, _)| {
                table == "devices"
                    && from == "redeemed_by_device"
                    && to == "id"
                    && on_delete == "CASCADE"
            }));

        let indexes: Vec<(i64, String, i64, String, i64)> =
            sqlx::query_as("PRAGMA index_list('join_codes')")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(indexes.iter().any(|(_, name, unique, _, partial)| {
            name == "idx_join_codes_unredeemed_code" && *unique == 1 && *partial == 1
        }));

        remove_test_database(&path, pool).await;
    }
}
