//! Media library engine for the SWARM server app (Phase 2+).
//!
//! Planned modules, porting the Batocera.Drone movies/music store
//! architecture (`movies_store.py` / `music_store.py`):
//! - `scan` — extension-allowlist walk, (size, mtime, fingerprint) change
//!   detection.
//! - `store` — SQLite (sqlx) library cache: entries + pending-changes queue +
//!   deleted-archive + whole-library thumbprint (the delta-sync primitive).
//! - `tags` — embedded metadata via lofty (ID3/Vorbis/MP4) — display overlay
//!   only; grouping keys stay path-derived.
//! - `probe` — ffprobe codec/container capture at scan time (feeds
//!   direct-play decisions).
//! - `scrape` — TMDb (user key) + MusicBrainz/Cover Art Archive/Wikimedia
//!   (keyless), with the inherited job discipline: NotFound(Unavailable)
//!   two-tier errors, Retry-After capped, SQLite-row-backed one-shot bulk
//!   jobs.
//! - `serve` — Range-aware direct-play serving over peer QUIC streams.
//! - `transcode` — ffmpeg HLS sessions with ABR ladder, seek-into-transcode,
//!   janitor eviction (evolution of `cast_stream.py`).

pub use swarm_core as core;
