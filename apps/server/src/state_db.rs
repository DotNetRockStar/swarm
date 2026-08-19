//! Server-local relational state: which STUN server/swarms this device is
//! linked to, and the periodic real upload-bandwidth measurement history
//! (see `bandwidth.rs`). SQLite via sqlx, same idempotent
//! `CREATE TABLE IF NOT EXISTS` + additive-`ensure_column` convention
//! `swarm-media::store::Library` already uses — not a separate migration
//! tool, kept consistent with the one already established in this
//! workspace. Replaces the old plain `stun-link.json` file: this data was
//! always structured (one link, many swarms) and a flat JSON blob couldn't
//! express that relation or be queried — `stun_link` is a singleton row
//! (`id` always 1, matching `swarm_media::store`'s style for "the current
//! X" rows) and `stun_link_swarm` is its one-to-many child, FK'd with
//! `ON DELETE CASCADE` so relinking to a different server can't leave
//! orphaned swarm rows behind.
//!
//! The access token itself still lives in `TokenStore` (OS keyring or
//! encrypted file), never here — this file is not a secret and is fine to
//! read while debugging, same posture the old `stun_link.rs` documented.

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::path::Path;
use std::str::FromStr;
use swarm_core::rest::SwarmSummary;

const STUN_LINK_ROW_ID: i64 = 1;

#[derive(Debug, Clone, PartialEq)]
pub struct StunLinkRecord {
    pub base_url: String,
    pub device_id: String,
    pub swarms: Vec<SwarmSummary>,
}

pub struct StateDb {
    pool: SqlitePool,
}

impl StateDb {
    pub async fn open(data_dir: &Path) -> sqlx::Result<Self> {
        let path = data_dir.join("server-state.sqlite");
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.to_str().unwrap_or_default()))?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::from_secs(5));
        let pool = SqlitePoolOptions::new().max_connections(4).connect_with(options).await?;
        sqlx::query("PRAGMA foreign_keys = ON;").execute(&pool).await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS stun_link (
                id INTEGER PRIMARY KEY,
                base_url TEXT NOT NULL,
                device_id TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS stun_link_swarm (
                id TEXT PRIMARY KEY,
                stun_link_id INTEGER NOT NULL REFERENCES stun_link(id) ON DELETE CASCADE,
                name TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_stun_link_swarm_link ON stun_link_swarm(stun_link_id);
            CREATE TABLE IF NOT EXISTS bandwidth_measurement (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                measured_at INTEGER NOT NULL,
                upload_bps INTEGER NOT NULL
            );
            "#,
        )
        .execute(&pool)
        .await?;
        Ok(Self { pool })
    }

    pub async fn load_stun_link(&self) -> sqlx::Result<Option<StunLinkRecord>> {
        let Some((base_url, device_id)): Option<(String, String)> =
            sqlx::query_as("SELECT base_url, device_id FROM stun_link WHERE id = ?")
                .bind(STUN_LINK_ROW_ID)
                .fetch_optional(&self.pool)
                .await?
        else {
            return Ok(None);
        };
        let swarms: Vec<(String, String)> =
            sqlx::query_as("SELECT id, name FROM stun_link_swarm WHERE stun_link_id = ? ORDER BY name")
                .bind(STUN_LINK_ROW_ID)
                .fetch_all(&self.pool)
                .await?;
        Ok(Some(StunLinkRecord {
            base_url,
            device_id,
            swarms: swarms.into_iter().map(|(id, name)| SwarmSummary { id, name }).collect(),
        }))
    }

    pub async fn save_stun_link(&self, record: &StunLinkRecord) -> sqlx::Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO stun_link (id, base_url, device_id, updated_at) VALUES (?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET base_url = excluded.base_url, device_id = excluded.device_id, updated_at = excluded.updated_at",
        )
        .bind(STUN_LINK_ROW_ID)
        .bind(&record.base_url)
        .bind(&record.device_id)
        .bind(now())
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM stun_link_swarm WHERE stun_link_id = ?")
            .bind(STUN_LINK_ROW_ID)
            .execute(&mut *tx)
            .await?;
        for swarm in &record.swarms {
            sqlx::query("INSERT INTO stun_link_swarm (id, stun_link_id, name) VALUES (?, ?, ?)")
                .bind(&swarm.id)
                .bind(STUN_LINK_ROW_ID)
                .bind(&swarm.name)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await
    }

    pub async fn record_bandwidth_measurement(&self, upload_bps: u64) -> sqlx::Result<()> {
        sqlx::query("INSERT INTO bandwidth_measurement (measured_at, upload_bps) VALUES (?, ?)")
            .bind(now())
            .bind(upload_bps as i64)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// The most recent successful measurement, if any — the auto-measured
    /// baseline `bandwidth.rs`'s periodic loop restores on startup so a
    /// restart doesn't fall back to the static default for a full interval.
    pub async fn latest_bandwidth_measurement(&self) -> sqlx::Result<Option<u64>> {
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT upload_bps FROM bandwidth_measurement ORDER BY id DESC LIMIT 1")
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(bps,)| bps as u64))
    }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stun_link_round_trips_with_its_swarms() {
        let dir = std::env::temp_dir().join(format!("swarm-state-db-link-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = StateDb::open(&dir).await.unwrap();
        assert!(db.load_stun_link().await.unwrap().is_none());

        let record = StunLinkRecord {
            base_url: "https://swarm.example.com".into(),
            device_id: "dev-1".into(),
            swarms: vec![
                SwarmSummary { id: "sw-1".into(), name: "Home".into() },
                SwarmSummary { id: "sw-2".into(), name: "Cabin".into() },
            ],
        };
        db.save_stun_link(&record).await.unwrap();
        let loaded = db.load_stun_link().await.unwrap().unwrap();
        assert_eq!(loaded.device_id, "dev-1");
        assert_eq!(loaded.swarms.len(), 2);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A relink (leaving one swarm, or replacing the whole link with a
    /// fresh one) must not leave orphaned `stun_link_swarm` rows behind —
    /// confirms the ON DELETE CASCADE / explicit delete-then-reinsert
    /// actually prevents drift between the two tables.
    #[tokio::test]
    async fn saving_a_smaller_swarm_list_drops_the_removed_rows() {
        let dir = std::env::temp_dir().join(format!("swarm-state-db-shrink-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = StateDb::open(&dir).await.unwrap();

        let mut record = StunLinkRecord {
            base_url: "https://swarm.example.com".into(),
            device_id: "dev-1".into(),
            swarms: vec![
                SwarmSummary { id: "sw-1".into(), name: "Home".into() },
                SwarmSummary { id: "sw-2".into(), name: "Cabin".into() },
            ],
        };
        db.save_stun_link(&record).await.unwrap();
        record.swarms.retain(|s| s.id != "sw-2");
        db.save_stun_link(&record).await.unwrap();

        let loaded = db.load_stun_link().await.unwrap().unwrap();
        assert_eq!(loaded.swarms.len(), 1);
        assert_eq!(loaded.swarms[0].id, "sw-1");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn latest_bandwidth_measurement_is_the_most_recent_row() {
        let dir = std::env::temp_dir().join(format!("swarm-state-db-bw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = StateDb::open(&dir).await.unwrap();
        assert_eq!(db.latest_bandwidth_measurement().await.unwrap(), None);

        db.record_bandwidth_measurement(5_000_000).await.unwrap();
        db.record_bandwidth_measurement(7_500_000).await.unwrap();
        assert_eq!(db.latest_bandwidth_measurement().await.unwrap(), Some(7_500_000));

        std::fs::remove_dir_all(&dir).ok();
    }
}
