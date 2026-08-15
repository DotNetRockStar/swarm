//! SQLite library store — the Batocera.Drone media-store architecture in
//! Rust: entries + pending-changes queue + deleted-archive + whole-library
//! thumbprint (the delta-sync/library-version primitive). Schema is created
//! idempotently; never bump applied schema in place.

use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::str::FromStr;
use swarm_core::peer::{AudioStreamInfo, CatalogEntry, MediaKind, VideoStreamInfo};

#[derive(Debug, Clone, PartialEq)]
pub struct EntryRecord {
    pub entry_key: String,
    pub relative_path: String,
    pub kind: MediaKind,
    pub title: String,
    pub size: u64,
    pub modified_time: i64,
    pub fingerprint: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub track_number: Option<u32>,
    pub show_title: Option<String>,
    pub season: Option<u32>,
    pub episode: Option<u32>,
    pub duration_secs: Option<f64>,
    pub video: Option<VideoStreamInfo>,
    pub audio: Option<AudioStreamInfo>,
    /// Scraper overlay — display-only, never a grouping key (Drone rule).
    pub scraped_title: Option<String>,
    pub genres: Vec<String>,
    pub artwork_version: u32,
}

/// Which artwork slot a downloaded image fills. Maps 1:1 onto the
/// `/art/{entry_key}/{kind}` peer route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtworkKind {
    Poster,
    Backdrop,
    Cover,
    ArtistPhoto,
}

impl ArtworkKind {
    pub fn route_segment(self) -> &'static str {
        match self {
            ArtworkKind::Poster => "poster",
            ArtworkKind::Backdrop => "backdrop",
            ArtworkKind::Cover => "cover",
            ArtworkKind::ArtistPhoto => "artist",
        }
    }

    pub fn parse(segment: &str) -> Option<Self> {
        match segment {
            "poster" => Some(ArtworkKind::Poster),
            "backdrop" => Some(ArtworkKind::Backdrop),
            "cover" => Some(ArtworkKind::Cover),
            "artist" => Some(ArtworkKind::ArtistPhoto),
            _ => None,
        }
    }

    fn column(self) -> &'static str {
        match self {
            ArtworkKind::Poster => "poster_relative_path",
            ArtworkKind::Backdrop => "backdrop_relative_path",
            ArtworkKind::Cover => "cover_relative_path",
            ArtworkKind::ArtistPhoto => "artist_art_relative_path",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingChange {
    pub entry_key: String,
    pub operation: String,
}

fn kind_str(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Movie => "movie",
        MediaKind::Episode => "episode",
        MediaKind::Track => "track",
    }
}

fn parse_kind(raw: &str) -> MediaKind {
    match raw {
        "episode" => MediaKind::Episode,
        "track" => MediaKind::Track,
        _ => MediaKind::Movie,
    }
}

pub struct Library {
    pool: SqlitePool,
}

impl Library {
    pub async fn open(database_path: &str) -> sqlx::Result<Self> {
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{database_path}"))?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::from_secs(5));
        let pool = SqlitePoolOptions::new().max_connections(4).connect_with(options).await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS library_entries (
                entry_key TEXT PRIMARY KEY,
                relative_path TEXT NOT NULL UNIQUE,
                kind TEXT NOT NULL,
                title TEXT NOT NULL,
                size INTEGER NOT NULL,
                modified_time INTEGER NOT NULL,
                fingerprint TEXT NOT NULL,
                artist TEXT, album TEXT, track_number INTEGER,
                show_title TEXT, season INTEGER, episode INTEGER,
                duration_secs REAL, video_json TEXT, audio_json TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_entries_path ON library_entries(relative_path COLLATE NOCASE);
            CREATE TABLE IF NOT EXISTS deleted_library_entries (
                entry_key TEXT PRIMARY KEY,
                relative_path TEXT NOT NULL,
                fingerprint TEXT NOT NULL,
                deleted_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS library_changes (
                entry_key TEXT PRIMARY KEY,
                operation TEXT NOT NULL CHECK (operation IN ('upsert','delete'))
            );
            "#,
        )
        .execute(&pool)
        .await?;
        // Scraper/artwork columns, added the idempotent way (never bump the
        // base CREATE TABLE in place — the Drone convention).
        for (column, ddl_type) in [
            ("scraped_title", "TEXT"),
            ("genres_json", "TEXT"),
            ("poster_relative_path", "TEXT"),
            ("backdrop_relative_path", "TEXT"),
            ("cover_relative_path", "TEXT"),
            ("artist_art_relative_path", "TEXT"),
            ("artwork_version", "INTEGER NOT NULL DEFAULT 0"),
        ] {
            ensure_column(&pool, "library_entries", column, ddl_type).await?;
        }
        Ok(Self { pool })
    }

    /// (size, modified_time, fingerprint) per relative path — the scanner's
    /// change-detection snapshot.
    pub async fn snapshot(&self) -> sqlx::Result<HashMap<String, (u64, i64, String)>> {
        let rows: Vec<(String, i64, i64, String)> =
            sqlx::query_as("SELECT relative_path, size, modified_time, fingerprint FROM library_entries")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows
            .into_iter()
            .map(|(path, size, mtime, fingerprint)| (path, (size as u64, mtime, fingerprint)))
            .collect())
    }

    pub async fn upsert(&self, record: &EntryRecord) -> sqlx::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO library_entries
                (entry_key, relative_path, kind, title, size, modified_time, fingerprint,
                 artist, album, track_number, show_title, season, episode,
                 duration_secs, video_json, audio_json)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(entry_key) DO UPDATE SET
                relative_path = excluded.relative_path, kind = excluded.kind, title = excluded.title,
                size = excluded.size, modified_time = excluded.modified_time, fingerprint = excluded.fingerprint,
                artist = excluded.artist, album = excluded.album, track_number = excluded.track_number,
                show_title = excluded.show_title, season = excluded.season, episode = excluded.episode,
                duration_secs = excluded.duration_secs, video_json = excluded.video_json,
                audio_json = excluded.audio_json
            "#,
        )
        .bind(&record.entry_key)
        .bind(&record.relative_path)
        .bind(kind_str(record.kind))
        .bind(&record.title)
        .bind(record.size as i64)
        .bind(record.modified_time)
        .bind(&record.fingerprint)
        .bind(&record.artist)
        .bind(&record.album)
        .bind(record.track_number.map(|n| n as i64))
        .bind(&record.show_title)
        .bind(record.season.map(|n| n as i64))
        .bind(record.episode.map(|n| n as i64))
        .bind(record.duration_secs)
        .bind(record.video.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default()))
        .bind(record.audio.as_ref().map(|a| serde_json::to_string(a).unwrap_or_default()))
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "INSERT INTO library_changes (entry_key, operation) VALUES (?, 'upsert') \
             ON CONFLICT(entry_key) DO UPDATE SET operation = 'upsert'",
        )
        .bind(&record.entry_key)
        .execute(&self.pool)
        .await?;
        // A resurrected path is no longer deleted.
        sqlx::query("DELETE FROM deleted_library_entries WHERE entry_key = ?")
            .bind(&record.entry_key)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn remove_by_path(&self, relative_path: &str) -> sqlx::Result<()> {
        let row: Option<(String, String)> =
            sqlx::query_as("SELECT entry_key, fingerprint FROM library_entries WHERE relative_path = ?")
                .bind(relative_path)
                .fetch_optional(&self.pool)
                .await?;
        let Some((entry_key, fingerprint)) = row else {
            return Ok(());
        };
        sqlx::query("DELETE FROM library_entries WHERE entry_key = ?")
            .bind(&entry_key)
            .execute(&self.pool)
            .await?;
        sqlx::query(
            "INSERT OR REPLACE INTO deleted_library_entries (entry_key, relative_path, fingerprint, deleted_at) \
             VALUES (?, ?, ?, strftime('%s','now'))",
        )
        .bind(&entry_key)
        .bind(relative_path)
        .bind(&fingerprint)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "INSERT INTO library_changes (entry_key, operation) VALUES (?, 'delete') \
             ON CONFLICT(entry_key) DO UPDATE SET operation = 'delete'",
        )
        .bind(&entry_key)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(&self, entry_key: &str) -> sqlx::Result<Option<EntryRecord>> {
        let row = sqlx::query_as::<_, EntryRow>(&format!("{ENTRY_SELECT} WHERE entry_key = ?"))
            .bind(entry_key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(EntryRecord::from))
    }

    pub async fn list(&self) -> sqlx::Result<Vec<EntryRecord>> {
        let rows = sqlx::query_as::<_, EntryRow>(&format!("{ENTRY_SELECT} ORDER BY relative_path"))
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(EntryRecord::from).collect())
    }

    pub async fn pending_changes(&self) -> sqlx::Result<Vec<PendingChange>> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT entry_key, operation FROM library_changes ORDER BY entry_key")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(entry_key, operation)| PendingChange { entry_key, operation }).collect())
    }

    pub async fn clear_pending_changes(&self) -> sqlx::Result<()> {
        sqlx::query("DELETE FROM library_changes").execute(&self.pool).await?;
        Ok(())
    }

    /// Whole-library version token: SHA-256 over the sorted
    /// `(lowercased path, fingerprint, size)` triples.
    pub async fn thumbprint(&self) -> sqlx::Result<String> {
        let mut rows: Vec<(String, String, i64)> =
            sqlx::query_as("SELECT relative_path, fingerprint, size FROM library_entries")
                .fetch_all(&self.pool)
                .await?;
        rows.sort_by_key(|a| a.0.to_lowercase());
        let mut digest = Sha256::new();
        for (path, fingerprint, size) in rows {
            digest.update(path.to_lowercase().as_bytes());
            digest.update(b"|");
            digest.update(fingerprint.as_bytes());
            digest.update(b"|");
            digest.update(size.to_le_bytes());
            digest.update(b"\n");
        }
        Ok(hex::encode(digest.finalize()))
    }

    pub async fn entry_count(&self) -> sqlx::Result<u64> {
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM library_entries").fetch_one(&self.pool).await?;
        Ok(count as u64)
    }

    /// Entries a scraper has not yet resolved (`scraped_title IS NULL`),
    /// oldest-added first isn't tracked — path order is deterministic enough
    /// for a bulk job's iteration.
    pub async fn missing_scrape(&self) -> sqlx::Result<Vec<EntryRecord>> {
        let rows = sqlx::query_as::<_, EntryRow>(&format!("{ENTRY_SELECT} WHERE scraped_title IS NULL ORDER BY relative_path"))
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(EntryRecord::from).collect())
    }

    /// Record that a scrape attempt completed for this entry — whether or not
    /// it found a match. `scraped_title` is a *display overlay*, never a
    /// grouping key: `None` means "processed, no title override" (covers
    /// both "no online match" and "matched but nothing to override," e.g.
    /// music tracks, which get release-level genres without a title
    /// override — see the runner). The sentinel is stored as `''` rather
    /// than `NULL` specifically so [`Self::missing_scrape`]'s `IS NULL`
    /// filter treats "processed" as done and never re-queues it.
    pub async fn set_scrape_result(&self, entry_key: &str, scraped_title: Option<&str>, genres: &[String]) -> sqlx::Result<()> {
        let genres_json = serde_json::to_string(genres).unwrap_or_else(|_| "[]".into());
        sqlx::query("UPDATE library_entries SET scraped_title = ?, genres_json = ? WHERE entry_key = ?")
            .bind(scraped_title.unwrap_or(""))
            .bind(genres_json)
            .bind(entry_key)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Store a downloaded artwork image's path (relative to the media root,
    /// alongside the source file) and bump the version that backs its etag.
    pub async fn set_artwork(&self, entry_key: &str, kind: ArtworkKind, relative_path: &str) -> sqlx::Result<()> {
        let sql = format!(
            "UPDATE library_entries SET {} = ?, artwork_version = artwork_version + 1 WHERE entry_key = ?",
            kind.column()
        );
        sqlx::query(&sql).bind(relative_path).bind(entry_key).execute(&self.pool).await?;
        Ok(())
    }

    /// The stored path and version (etag material) for one artwork slot, if
    /// downloaded. One query, since `serve.rs` needs both together.
    pub async fn artwork(&self, entry_key: &str, kind: ArtworkKind) -> sqlx::Result<Option<(String, u32)>> {
        let sql = format!("SELECT {}, artwork_version FROM library_entries WHERE entry_key = ?", kind.column());
        let row: Option<(Option<String>, i64)> = sqlx::query_as(&sql).bind(entry_key).fetch_optional(&self.pool).await?;
        Ok(row.and_then(|(path, version)| path.map(|p| (p, version as u32))))
    }
}

/// `ALTER TABLE ADD COLUMN IF NOT EXISTS` doesn't exist in SQLite; check
/// `pragma_table_info` first so re-adding a column already present is a
/// silent no-op instead of an error.
async fn ensure_column(pool: &SqlitePool, table: &str, column: &str, ddl_type: &str) -> sqlx::Result<()> {
    let exists: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM pragma_table_info(?) WHERE name = ?")
        .bind(table)
        .bind(column)
        .fetch_one(pool)
        .await?;
    if exists.0 == 0 {
        sqlx::query(&format!("ALTER TABLE {table} ADD COLUMN {column} {ddl_type}")).execute(pool).await?;
    }
    Ok(())
}

const ENTRY_SELECT: &str =
    "SELECT entry_key, relative_path, kind, title, size, modified_time, fingerprint, artist, album, \
     track_number, show_title, season, episode, duration_secs, video_json, audio_json, \
     scraped_title, genres_json, artwork_version FROM library_entries";

#[derive(sqlx::FromRow)]
struct EntryRow {
    entry_key: String,
    relative_path: String,
    kind: String,
    title: String,
    size: i64,
    modified_time: i64,
    fingerprint: String,
    artist: Option<String>,
    album: Option<String>,
    track_number: Option<i64>,
    show_title: Option<String>,
    season: Option<i64>,
    episode: Option<i64>,
    duration_secs: Option<f64>,
    video_json: Option<String>,
    audio_json: Option<String>,
    scraped_title: Option<String>,
    genres_json: Option<String>,
    artwork_version: i64,
}

impl From<EntryRow> for EntryRecord {
    fn from(row: EntryRow) -> Self {
        EntryRecord {
            entry_key: row.entry_key,
            relative_path: row.relative_path,
            kind: parse_kind(&row.kind),
            title: row.title,
            size: row.size as u64,
            modified_time: row.modified_time,
            fingerprint: row.fingerprint,
            artist: row.artist,
            album: row.album,
            track_number: row.track_number.map(|n| n as u32),
            show_title: row.show_title,
            season: row.season.map(|n| n as u32),
            episode: row.episode.map(|n| n as u32),
            duration_secs: row.duration_secs,
            video: row.video_json.as_deref().and_then(|j| serde_json::from_str(j).ok()),
            audio: row.audio_json.as_deref().and_then(|j| serde_json::from_str(j).ok()),
            // Empty string is the "scraped, no match found" marker (see
            // set_scrape_not_found) — surface it as no display overlay.
            scraped_title: row.scraped_title.filter(|t| !t.is_empty()),
            genres: row.genres_json.as_deref().and_then(|j| serde_json::from_str(j).ok()).unwrap_or_default(),
            artwork_version: row.artwork_version as u32,
        }
    }
}

impl EntryRecord {
    pub fn to_catalog_entry(&self) -> CatalogEntry {
        CatalogEntry {
            entry_key: self.entry_key.clone(),
            fingerprint: self.fingerprint.clone(),
            kind: self.kind,
            title: self.title.clone(),
            size: self.size,
            duration_secs: self.duration_secs,
            show_title: self.show_title.clone(),
            season: self.season,
            episode: self.episode,
            artist: self.artist.clone(),
            album: self.album.clone(),
            track_number: self.track_number,
            scraped_title: self.scraped_title.clone(),
            genres: self.genres.clone(),
            video: self.video.clone(),
            audio: self.audio.clone(),
            artwork_etag: (self.artwork_version > 0).then(|| format!("v{}", self.artwork_version)),
        }
    }
}
