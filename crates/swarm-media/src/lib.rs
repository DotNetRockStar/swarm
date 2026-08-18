//! Media library engine for the SWARM server app.
//!
//! Implemented (Phase 2):
//! - [`classify`] — extension allowlist + path-derived grouping (movie /
//!   episode / track, artist/album, SxxEyy, disc-folder absorption).
//! - [`store`] — SQLite library: entries + pending-changes queue +
//!   deleted-archive + whole-library thumbprint (delta-sync primitive).
//! - [`scan`] — walk → (size, mtime) change detection → sample-fp-v1 →
//!   tags/probe enrichment → store reconciliation.
//! - [`tags`] — embedded tags via lofty (display overlay only).
//! - [`probe`] — optional ffprobe codec/duration capture for direct-play
//!   decisions.
//! - [`range`] — HTTP-semantics byte-range resolution + content types.
//! - [`serve`] — the peer-facing media service over QUIC streams, including
//!   the `/art/*` route.
//! - [`scrape`] — TMDb/MusicBrainz/Cover Art Archive/Wikimedia metadata and
//!   artwork, with the inherited two-tier-error job discipline.
//!
//! - [`transcode`] — upload-budgeted direct/HLS playback sessions backed by
//!   FFmpeg, with an adaptive H.264/AAC ladder and idle cleanup.

pub mod classify;
pub mod probe;
pub mod range;
pub mod roots;
pub mod scan;
pub mod scrape;
pub mod serve;
pub mod store;
pub mod tags;
pub mod transcode;

pub use swarm_core as core;
