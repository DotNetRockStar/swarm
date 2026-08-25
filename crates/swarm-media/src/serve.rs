//! The peer-facing media service: maps `PeerRequest`s onto the library and
//! the filesystem, and the QUIC accept loop that runs it.
//!
//! Path safety follows the Drone discipline: entry keys are validated as
//! lowercase hex *before* any lookup, and the file path served always comes
//! from the library row (derived from the scanned relative path under the
//! media root) — never from request input.

use crate::bandwidth::BandwidthMeter;
use crate::range::{content_type, resolve, ResolvedRange};
use crate::roots::{RootResolver, SharedRootResolver};
use crate::store::{ArtworkKind, Library};
use crate::transcode::{
    hls_content_type, SessionRateLimiter, TranscodeConfig, TranscodeError, TranscodeManager,
};
use bytes::Bytes;
use flate2::write::GzEncoder;
use flate2::Compression;
use futures_util::stream::{self, Stream};
use std::io::BufWriter;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use swarm_core::entry_key::is_valid_entry_key;
use swarm_core::peer::{
    CatalogManifest, CatalogThumbprint, PeerRequest, PeerResponseHeader, SubtitleTrack,
};
use swarm_p2p::endpoint::{read_request, write_response_header, P2pError};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

pub struct MediaService {
    library: Arc<Library>,
    roots: SharedRootResolver,
    transcodes: Arc<TranscodeManager>,
    thumbnail_generation: tokio::sync::Mutex<()>,
    artwork_cache_dir: Option<PathBuf>,
    artwork_cache_enabled: AtomicBool,
    artwork_cache_fills: [tokio::sync::Mutex<()>; 32],
    bandwidth: Arc<BandwidthMeter>,
}

const ARTWORK_CACHE_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// A resolved response: header plus a body source the transport streams out.
pub enum Body {
    Bytes(Vec<u8>),
    File {
        path: PathBuf,
        offset: u64,
        len: u64,
        rate_limiters: Vec<Arc<SessionRateLimiter>>,
    },
}

pub struct Resolved {
    pub header: PeerResponseHeader,
    pub body: Body,
    /// Playback session held in-use until `handle_stream` finishes writing.
    session_id: Option<String>,
}

fn status(status: u16) -> Resolved {
    Resolved {
        header: PeerResponseHeader {
            status,
            len: 0,
            content_type: None,
            content_range: None,
            etag: None,
        },
        body: Body::Bytes(Vec::new()),
        session_id: None,
    }
}

fn json_response(status: u16, value: &impl serde::Serialize) -> Resolved {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    Resolved {
        header: PeerResponseHeader {
            status,
            len: bytes.len() as u64,
            content_type: Some("application/json".into()),
            content_range: None,
            etag: None,
        },
        body: Body::Bytes(bytes),
        session_id: None,
    }
}

fn gzip_json_response(status: u16, value: &impl serde::Serialize) -> Resolved {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    let bytes = serde_json::to_writer(&mut encoder, value)
        .and_then(|_| encoder.finish().map_err(serde_json::Error::io))
        .unwrap_or_default();
    Resolved {
        header: PeerResponseHeader {
            status,
            len: bytes.len() as u64,
            content_type: Some("application/gzip".into()),
            content_range: None,
            etag: None,
        },
        body: Body::Bytes(bytes),
        session_id: None,
    }
}

impl MediaService {
    pub fn new(library: Arc<Library>, media_root: PathBuf) -> Self {
        let config = TranscodeConfig::disabled(std::env::temp_dir().join("swarm-hls-disabled"));
        Self::with_transcoding(library, media_root, config)
    }

    pub fn with_transcoding(
        library: Arc<Library>,
        media_root: PathBuf,
        config: TranscodeConfig,
    ) -> Self {
        Self::with_roots(
            library,
            SharedRootResolver::new(RootResolver::single(media_root)),
            config,
        )
    }

    /// Multi-root variant of [`Self::with_transcoding`] — see `crate::roots`.
    /// Takes a [`SharedRootResolver`] (not a bare [`RootResolver`]) so a
    /// caller that later live-updates its roots (see
    /// `ServerCore::update_media_roots`) can share the exact same handle
    /// with this service — a bare `RootResolver` clone would silently drift
    /// out of sync on the next update.
    pub fn with_roots(
        library: Arc<Library>,
        roots: SharedRootResolver,
        config: TranscodeConfig,
    ) -> Self {
        Self::with_optional_artwork_cache(library, roots, config, None)
    }

    /// Construct a service whose optional artwork cache lives on the media
    /// server's local disk. Caching remains off until
    /// [`Self::set_artwork_disk_cache_enabled`] is called, matching the
    /// persisted desktop preference's opt-in behavior.
    pub fn with_roots_and_artwork_cache(
        library: Arc<Library>,
        roots: SharedRootResolver,
        config: TranscodeConfig,
        artwork_cache_dir: PathBuf,
    ) -> Self {
        Self::with_optional_artwork_cache(library, roots, config, Some(artwork_cache_dir))
    }

    fn with_optional_artwork_cache(
        library: Arc<Library>,
        roots: SharedRootResolver,
        config: TranscodeConfig,
        artwork_cache_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            library,
            roots,
            transcodes: TranscodeManager::new(config),
            thumbnail_generation: tokio::sync::Mutex::new(()),
            artwork_cache_dir,
            artwork_cache_enabled: AtomicBool::new(false),
            artwork_cache_fills: std::array::from_fn(|_| tokio::sync::Mutex::new(())),
            bandwidth: BandwidthMeter::new(),
        }
    }

    pub fn set_artwork_disk_cache_enabled(&self, enabled: bool) {
        self.artwork_cache_enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn transcode_manager(&self) -> &Arc<TranscodeManager> {
        &self.transcodes
    }

    pub fn bandwidth_meter(&self) -> &Arc<BandwidthMeter> {
        &self.bandwidth
    }

    pub async fn resolve(&self, request: &PeerRequest) -> Resolved {
        self.resolve_for_network(request, false).await
    }

    /// Resolve a request with transport context. `is_lan` bypasses upload
    /// admission limits and pacing because a local transfer does not spend
    /// the internet uplink the budget is meant to protect.
    pub async fn resolve_for_network(&self, request: &PeerRequest, is_lan: bool) -> Resolved {
        let request_path = request
            .path
            .split_once('?')
            .map(|(path, _)| path)
            .unwrap_or(request.path.as_str());
        match request_path {
            "/catalog/thumbprint" => self.thumbprint().await,
            "/catalog/manifest" => self.manifest(false).await,
            "/catalog/manifest.gz" => self.manifest(true).await,
            "/errors/report" => self.report_error(request).await,
            "/likes/toggle" => self.set_like(request).await,
            path => {
                if let Some(entry_key) = path.strip_prefix("/media/") {
                    self.media(entry_key, request, is_lan).await
                } else if let Some(entry_key) = path.strip_prefix("/play/") {
                    self.play(entry_key, request, is_lan).await
                } else if let Some(rest) = path.strip_prefix("/stream/") {
                    self.session_media(rest, request, is_lan).await
                } else if let Some(rest) = path.strip_prefix("/hls/") {
                    self.hls(rest, request, is_lan).await
                } else if let Some(session_id) = path.strip_prefix("/stop/") {
                    self.stop(session_id).await
                } else if let Some(rest) = path.strip_prefix("/subtitles/") {
                    self.subtitle(rest).await
                } else if let Some(rest) = path.strip_prefix("/art/") {
                    let mut segments = rest.splitn(2, '/');
                    let entry_key = segments.next().unwrap_or("");
                    let kind = segments.next().unwrap_or("");
                    self.art(entry_key, kind, request).await
                } else {
                    status(404)
                }
            }
        }
    }

    async fn thumbprint(&self) -> Resolved {
        match self.library.catalog_snapshot().await {
            Ok((thumbprint, entries)) => json_response(
                200,
                &CatalogThumbprint {
                    thumbprint,
                    entry_count: entries.len() as u64,
                },
            ),
            _ => status(500),
        }
    }

    async fn manifest(&self, compressed: bool) -> Resolved {
        let Ok((thumbprint, entries)) = self.library.catalog_snapshot().await else {
            return status(500);
        };
        let manifest = CatalogManifest {
            thumbprint,
            entries,
            removed: Vec::new(),
        };
        if compressed {
            gzip_json_response(200, &manifest)
        } else {
            json_response(200, &manifest)
        }
    }

    /// `/errors/report` — a client persists a [`swarm_core::peer::ClientErrorReport`]
    /// here for later triage on this server's own swarm page, rather than it
    /// only ever existing in on-device logs nobody's looking at.
    async fn report_error(&self, request: &PeerRequest) -> Resolved {
        let Some(report) = &request.error_report else {
            return status(400);
        };
        if report.device_id.is_empty() || report.message.is_empty() {
            return status(400);
        }
        match self.library.record_client_error(report).await {
            Ok(()) => status(204),
            Err(_) => status(500),
        }
    }

    /// `/likes/toggle` — see [`swarm_core::peer::LikeToggle`]'s doc comment
    /// for the idempotent-desired-end-state semantics.
    async fn set_like(&self, request: &PeerRequest) -> Resolved {
        let Some(like) = &request.like else {
            return status(400);
        };
        if like.device_id.is_empty() || like.entry_key.is_empty() {
            return status(400);
        }
        match self
            .library
            .set_like(
                &like.entry_key,
                &like.device_id,
                &like.device_name,
                like.liked,
            )
            .await
        {
            Ok(()) => status(204),
            Err(_) => status(500),
        }
    }

    async fn media(&self, entry_key: &str, request: &PeerRequest, is_lan: bool) -> Resolved {
        if !is_valid_entry_key(entry_key) {
            return status(404);
        }
        let Ok(Some(entry)) = self.library.get(entry_key).await else {
            return status(404);
        };
        self.media_entry(entry, request, None, self.rate_limiters(is_lan, None))
            .await
    }

    async fn media_entry(
        &self,
        entry: crate::store::EntryRecord,
        request: &PeerRequest,
        session_id: Option<String>,
        rate_limiters: Vec<Arc<SessionRateLimiter>>,
    ) -> Resolved {
        let path = self.roots.resolve(&entry.relative_path);
        let Ok(metadata) = std::fs::metadata(&path) else {
            return status(404); // deleted since last scan
        };
        let total = metadata.len();
        match resolve(request.range, total) {
            ResolvedRange::Full { len } => Resolved {
                header: PeerResponseHeader {
                    status: 200,
                    len,
                    content_type: Some(content_type(&entry.relative_path).into()),
                    content_range: None,
                    etag: Some(entry.fingerprint.clone()),
                },
                body: Body::File {
                    path,
                    offset: 0,
                    len,
                    rate_limiters,
                },
                session_id,
            },
            ResolvedRange::Partial(content_range) => {
                let len = content_range.end - content_range.start + 1;
                Resolved {
                    header: PeerResponseHeader {
                        status: 206,
                        len,
                        content_type: Some(content_type(&entry.relative_path).into()),
                        content_range: Some(content_range),
                        etag: Some(entry.fingerprint.clone()),
                    },
                    body: Body::File {
                        path,
                        offset: content_range.start,
                        len,
                        rate_limiters,
                    },
                    session_id,
                }
            }
            ResolvedRange::Unsatisfiable => status(416),
        }
    }

    async fn play(&self, entry_key: &str, request: &PeerRequest, is_lan: bool) -> Resolved {
        if !is_valid_entry_key(entry_key) {
            return status(404);
        }
        let Some(preferences) = request.playback.as_ref() else {
            return transcode_error(TranscodeError::MissingPreferences);
        };
        let Ok(Some(entry)) = self.library.get(entry_key).await else {
            return status(404);
        };
        let media_path = self.roots.resolve(&entry.relative_path);
        if !media_path.is_file() {
            return status(404);
        }
        match self
            .transcodes
            .plan(&entry, &media_path, preferences, is_lan)
            .await
        {
            Ok(mut plan) => {
                if entry.kind == swarm_core::peer::MediaKind::Track {
                    match self.library.track_lyrics(entry_key).await {
                        Ok(lyrics) => plan.lyrics = lyrics,
                        Err(error) => {
                            tracing::warn!(entry_key, %error, "could not load cached lyrics for playback");
                        }
                    }
                } else if !preferences.preview {
                    match self.library.subtitle_tracks(entry_key).await {
                        Ok(tracks) => {
                            plan.subtitles = tracks
                                .into_iter()
                                .filter(|track| track.fingerprint == entry.fingerprint)
                                .filter(|track| PathBuf::from(&track.file_path).is_file())
                                .map(|track| SubtitleTrack {
                                    path: format!("/subtitles/{entry_key}/{}.vtt", track.id),
                                    id: track.id,
                                    language: track.language,
                                    label: track.label,
                                    source: track.source,
                                })
                                .collect();
                        }
                        Err(error) => {
                            tracing::warn!(entry_key, %error, "could not load generated subtitles for playback");
                        }
                    }
                }
                json_response(200, &plan)
            }
            Err(error) => {
                tracing::warn!(entry_key, %error, "playback negotiation failed");
                transcode_error(error)
            }
        }
    }

    /// Explicit early release of a playback session's bandwidth reservation
    /// (player screen torn down — back-press or moving to the next entry).
    /// Idempotent and always 200, including for an id that already expired
    /// or was never valid: the client fires this best-effort on its way out
    /// and has no useful recovery if the server disagrees about whether the
    /// session still existed.
    async fn stop(&self, session_id: &str) -> Resolved {
        self.transcodes.release(session_id);
        status(200)
    }

    /// Serve only a completed track previously registered in SQLite. The
    /// request never becomes a filesystem path, so this cannot traverse out
    /// of the server-managed subtitle directory.
    async fn subtitle(&self, rest: &str) -> Resolved {
        let Some((entry_key, filename)) = rest.split_once('/') else {
            return status(404);
        };
        if !is_valid_entry_key(entry_key) {
            return status(404);
        }
        let Some(track_id) = filename.strip_suffix(".vtt") else {
            return status(404);
        };
        let Ok(Some(track)) = self.library.subtitle_track(entry_key, track_id).await else {
            return status(404);
        };
        let Ok(Some(entry)) = self.library.get(entry_key).await else {
            return status(404);
        };
        if track.fingerprint != entry.fingerprint || track.format != "vtt" {
            return status(404);
        }
        match tokio::fs::read(&track.file_path).await {
            Ok(bytes) => Resolved {
                header: PeerResponseHeader {
                    status: 200,
                    len: bytes.len() as u64,
                    content_type: Some("text/vtt; charset=utf-8".into()),
                    content_range: None,
                    etag: None,
                },
                body: Body::Bytes(bytes),
                session_id: None,
            },
            Err(_) => status(404),
        }
    }

    async fn session_media(&self, rest: &str, request: &PeerRequest, is_lan: bool) -> Resolved {
        let Some((session_id, tail)) = rest.split_once('/') else {
            return status(404);
        };
        if tail != "media" {
            return status(404);
        }
        let Some((entry_key, rate_limiter)) = self.transcodes.open_direct(session_id) else {
            return status(404);
        };
        let Ok(Some(entry)) = self.library.get(&entry_key).await else {
            self.transcodes.finish_use(session_id);
            return status(404);
        };
        self.media_entry(
            entry,
            request,
            Some(session_id.to_string()),
            self.rate_limiters(is_lan, Some(rate_limiter)),
        )
        .await
    }

    async fn hls(&self, rest: &str, request: &PeerRequest, is_lan: bool) -> Resolved {
        let Some((session_id, relative_path)) = rest.split_once('/') else {
            return status(404);
        };
        let Some(file) = self.transcodes.open_hls(session_id, relative_path) else {
            return status(404);
        };
        let Ok(metadata) = std::fs::metadata(&file.path) else {
            self.transcodes.finish_use(session_id);
            return status(404);
        };
        let total = metadata.len();
        let content_type = hls_content_type(&file.path);
        match resolve(request.range, total) {
            ResolvedRange::Full { len } => Resolved {
                header: PeerResponseHeader {
                    status: 200,
                    len,
                    content_type: Some(content_type.into()),
                    content_range: None,
                    etag: None,
                },
                body: Body::File {
                    path: file.path,
                    offset: 0,
                    len,
                    rate_limiters: self.rate_limiters(is_lan, Some(file.rate_limiter)),
                },
                session_id: Some(file.session_id),
            },
            ResolvedRange::Partial(content_range) => {
                let len = content_range.end - content_range.start + 1;
                Resolved {
                    header: PeerResponseHeader {
                        status: 206,
                        len,
                        content_type: Some(content_type.into()),
                        content_range: Some(content_range),
                        etag: None,
                    },
                    body: Body::File {
                        path: file.path,
                        offset: content_range.start,
                        len,
                        rate_limiters: self.rate_limiters(is_lan, Some(file.rate_limiter)),
                    },
                    session_id: Some(file.session_id),
                }
            }
            ResolvedRange::Unsatisfiable => {
                self.transcodes.finish_use(session_id);
                status(416)
            }
        }
    }

    fn rate_limiters(
        &self,
        is_lan: bool,
        session: Option<Arc<SessionRateLimiter>>,
    ) -> Vec<Arc<SessionRateLimiter>> {
        if !self.transcodes.should_throttle(is_lan) {
            return Vec::new();
        }
        let mut limiters = vec![self.transcodes.global_rate_limiter()];
        if let Some(session) = session {
            limiters.push(session);
        }
        limiters
    }

    /// `GET /art/{entry_key}/{poster|season|backdrop|cover|artist}` — the artwork a
    /// scrape wrote, served the same way as media bytes (Range + etag), with
    /// `if_none_match` short-circuiting to 304 when the client already has
    /// the current version.
    async fn art(&self, entry_key: &str, kind_segment: &str, request: &PeerRequest) -> Resolved {
        if !is_valid_entry_key(entry_key) {
            return status(404);
        }
        let Some(kind) = ArtworkKind::parse(kind_segment) else {
            return status(404);
        };
        let Ok(Some((relative_path, version))) = self.library.artwork(entry_key, kind).await else {
            return status(404);
        };
        let requested_width = artwork_thumbnail_width(&request.path);
        let etag = requested_width.map_or_else(
            || format!("v{version}"),
            |width| format!("v{version}-w{width}"),
        );
        if request.if_none_match.as_deref() == Some(etag.as_str()) {
            return Resolved {
                header: PeerResponseHeader {
                    status: 304,
                    len: 0,
                    content_type: None,
                    content_range: None,
                    etag: Some(etag),
                },
                body: Body::Bytes(Vec::new()),
                session_id: None,
            };
        }
        let source_path = self.roots.resolve(&relative_path);
        let source_path = self
            .cached_artwork_path(&source_path, entry_key, kind.route_segment(), version)
            .await
            .unwrap_or(source_path);
        let path = match requested_width {
            Some(width) => self
                .thumbnail_path(
                    &source_path,
                    entry_key,
                    kind.route_segment(),
                    version,
                    width,
                )
                .await
                .unwrap_or(source_path),
            None => source_path,
        };
        let Ok(metadata) = std::fs::metadata(&path) else {
            return status(404); // artwork file missing from disk since the scrape
        };
        let total = metadata.len();
        match resolve(request.range, total) {
            ResolvedRange::Full { len } => Resolved {
                header: PeerResponseHeader {
                    status: 200,
                    len,
                    content_type: Some(image_content_type(path.to_string_lossy().as_ref()).into()),
                    content_range: None,
                    etag: Some(etag),
                },
                body: Body::File {
                    path,
                    offset: 0,
                    len,
                    rate_limiters: Vec::new(),
                },
                session_id: None,
            },
            ResolvedRange::Partial(content_range) => {
                let len = content_range.end - content_range.start + 1;
                Resolved {
                    header: PeerResponseHeader {
                        status: 206,
                        len,
                        content_type: Some(
                            image_content_type(path.to_string_lossy().as_ref()).into(),
                        ),
                        content_range: Some(content_range),
                        etag: Some(etag),
                    },
                    body: Body::File {
                        path,
                        offset: content_range.start,
                        len,
                        rate_limiters: Vec::new(),
                    },
                    session_id: None,
                }
            }
            ResolvedRange::Unsatisfiable => status(416),
        }
    }

    /// Resolve an artwork request through the server-local read-through
    /// cache. The library artwork version makes scrape/manual replacements
    /// immediately select a new cache key; the fixed TTL also refreshes a
    /// source file that was changed outside those managed workflows.
    async fn cached_artwork_path(
        &self,
        source: &std::path::Path,
        entry_key: &str,
        kind: &str,
        version: u32,
    ) -> Option<PathBuf> {
        if !self.artwork_cache_enabled.load(Ordering::Relaxed) {
            return None;
        }
        let cache_root = self.artwork_cache_dir.as_ref()?;
        let extension = source
            .extension()
            .and_then(|value| value.to_str())
            .filter(|value| {
                !value.is_empty()
                    && value.len() <= 10
                    && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
            })
            .unwrap_or("img")
            .to_ascii_lowercase();
        let shard = entry_key.get(..2).unwrap_or("00");
        let file_prefix = format!("{entry_key}-{kind}-");
        let target = cache_root
            .join(shard)
            .join(format!("{file_prefix}v{version}.{extension}"));
        if artwork_cache_file_is_fresh(&target) {
            return Some(target);
        }

        // Requests for the same key share a lock to avoid duplicate SMB reads;
        // unrelated misses can still fill concurrently during a large browse.
        let fill_index =
            usize::from_str_radix(shard, 16).unwrap_or(0) % self.artwork_cache_fills.len();
        let _fill = self.artwork_cache_fills[fill_index].lock().await;
        if artwork_cache_file_is_fresh(&target) {
            return Some(target);
        }

        let source = source.to_path_buf();
        let output = target.clone();
        let prefix = file_prefix.clone();
        let refreshed =
            tokio::task::spawn_blocking(move || fill_artwork_cache(&source, &output, &prefix))
                .await
                .ok()
                .and_then(Result::ok)
                .is_some();
        if refreshed || std::fs::metadata(&target).is_ok_and(|metadata| metadata.len() > 0) {
            Some(target)
        } else {
            None
        }
    }

    /// Build a persistent, version-keyed JPEG thumbnail beside the source
    /// artwork. Generation is serialized and performed on the blocking pool:
    /// image decode/resize/encode is CPU and filesystem work and must never
    /// occupy a Tokio request worker. Any failure falls back to the original
    /// file, so thumbnail support cannot make existing artwork unavailable.
    async fn thumbnail_path(
        &self,
        source: &std::path::Path,
        entry_key: &str,
        kind: &str,
        version: u32,
        width: u32,
    ) -> Option<PathBuf> {
        let parent = source.parent()?;
        let cache_dir = parent.join(".swarm-thumbnails");
        let file_prefix = format!("{entry_key}-{kind}-");
        let target = cache_dir.join(format!("{file_prefix}v{version}-w{width}.jpg"));
        if std::fs::metadata(&target).is_ok_and(|metadata| metadata.len() > 0) {
            return Some(target);
        }

        let _generation = self.thumbnail_generation.lock().await;
        if std::fs::metadata(&target).is_ok_and(|metadata| metadata.len() > 0) {
            return Some(target);
        }

        let source = source.to_path_buf();
        let output = target.clone();
        tokio::task::spawn_blocking(move || {
            generate_artwork_thumbnail(&source, &output, &file_prefix, width)
        })
        .await
        .ok()
        .and_then(Result::ok)
        .map(|_| target)
    }
}

fn artwork_cache_file_is_fresh(path: &std::path::Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| {
        metadata.len() > 0
            && metadata.modified().ok().is_some_and(|modified| {
                modified
                    .elapsed()
                    .map_or(true, |age| age < ARTWORK_CACHE_TTL)
            })
    })
}

fn fill_artwork_cache(
    source: &std::path::Path,
    target: &std::path::Path,
    file_prefix: &str,
) -> std::io::Result<()> {
    let cache_dir = target.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "artwork cache target has no parent",
        )
    })?;
    std::fs::create_dir_all(cache_dir)?;
    let filename = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("artwork");
    let temporary = cache_dir.join(format!(".{filename}.{}.tmp", std::process::id()));
    if let Err(error) = std::fs::copy(source, &temporary) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    if std::fs::metadata(&temporary)?.len() == 0 {
        let _ = std::fs::remove_file(&temporary);
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "artwork source is empty",
        ));
    }
    if let Err(error) = std::fs::rename(&temporary, target) {
        // Windows does not replace an existing destination. Only remove the
        // stale file after the complete replacement has been copied locally.
        if target.exists() {
            std::fs::remove_file(target)?;
            std::fs::rename(&temporary, target)?;
        } else {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
    }

    // Artwork-version changes invalidate immediately; removing superseded
    // files prevents repeated scrapes from growing the cache indefinitely.
    if let Ok(entries) = std::fs::read_dir(cache_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if name.to_string_lossy().starts_with(file_prefix) && path != target {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    Ok(())
}

fn artwork_thumbnail_width(path: &str) -> Option<u32> {
    let query = path.split_once('?')?.1;
    query.split('&').find_map(|part| {
        let (name, value) = part.split_once('=')?;
        if name != "w" {
            return None;
        }
        match value.parse::<u32>().ok()? {
            320 => Some(320),
            640 => Some(640),
            _ => None,
        }
    })
}

fn generate_artwork_thumbnail(
    source: &std::path::Path,
    target: &std::path::Path,
    file_prefix: &str,
    width: u32,
) -> Result<(), image::ImageError> {
    let image = image::ImageReader::open(source)?
        .with_guessed_format()?
        .decode()?;
    let target_height = ((image.height() as u64 * width as u64) / image.width().max(1) as u64)
        .clamp(1, 1280) as u32;
    let thumbnail = image.thumbnail(width, target_height);
    let cache_dir = target.parent().ok_or_else(|| {
        image::ImageError::IoError(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "thumbnail target has no parent",
        ))
    })?;
    std::fs::create_dir_all(cache_dir)?;

    let temporary = cache_dir.join(format!(".{file_prefix}{}.tmp", std::process::id()));
    {
        let file = std::fs::File::create(&temporary)?;
        let mut writer = BufWriter::new(file);
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, 82);
        encoder.encode_image(&thumbnail)?;
    }
    std::fs::rename(&temporary, target)?;

    // One current variant per entry/kind/size is enough. Removing old
    // version files keeps long-lived libraries from accumulating thumbnails
    // every time artwork is replaced.
    if let Ok(entries) = std::fs::read_dir(cache_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(file_prefix)
                && name.ends_with(&format!("-w{width}.jpg"))
                && path != target
            {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    Ok(())
}

fn transcode_error(error: TranscodeError) -> Resolved {
    let status_code = match &error {
        TranscodeError::Capacity | TranscodeError::Bandwidth => 429,
        TranscodeError::MissingPreferences => 400,
        _ => 503,
    };
    json_response(
        status_code,
        &serde_json::json!({ "error": error.to_string() }),
    )
}

fn image_content_type(relative_path: &str) -> &'static str {
    match relative_path
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "webp" => "image/webp",
        _ => "image/jpeg", // every scraper writes .jpg today
    }
}

/// Calls [`TranscodeManager::finish_use`] exactly once when dropped — on
/// natural stream completion (the final yielded [`BodyState`] is dropped)
/// and on early drop alike, since a caller abandoning a stream mid-read (an
/// HTTP client seeking or disconnecting mid-range-request, say) is routine,
/// not exceptional, and must not leak the session either way. Kept private:
/// [`Resolved::session_id`] is private for the same reason — nothing outside
/// [`stream_body`] should be able to forget to release it.
struct SessionGuard {
    manager: Arc<TranscodeManager>,
    session_id: Option<String>,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        if let Some(session_id) = self.session_id.take() {
            self.manager.finish_use(&session_id);
        }
    }
}

enum BodyState {
    Bytes {
        bytes: Bytes,
        guard: SessionGuard,
    },
    FilePending {
        path: PathBuf,
        offset: u64,
        remaining: u64,
        rate_limiters: Vec<Arc<SessionRateLimiter>>,
        bandwidth: Arc<BandwidthMeter>,
        guard: SessionGuard,
    },
    FileOpen {
        file: tokio::fs::File,
        remaining: u64,
        rate_limiters: Vec<Arc<SessionRateLimiter>>,
        bandwidth: Arc<BandwidthMeter>,
        guard: SessionGuard,
    },
    Finished {
        #[allow(dead_code)]
        guard: SessionGuard,
    },
}

/// Reads at most one 64 KiB chunk from `file`, applying every rate limiter
/// and recording bandwidth exactly as the QUIC transport always has.
/// `Ok(None)` means `remaining` was already zero — the body is exhausted.
async fn read_file_chunk(
    file: &mut tokio::fs::File,
    remaining: u64,
    rate_limiters: &[Arc<SessionRateLimiter>],
    bandwidth: &Arc<BandwidthMeter>,
) -> std::io::Result<Option<Bytes>> {
    if remaining == 0 {
        return Ok(None);
    }
    let want = (64 * 1024).min(remaining as usize);
    let mut buffer = vec![0u8; want];
    let got = file.read(&mut buffer).await?;
    if got == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "file truncated while serving",
        ));
    }
    for limiter in rate_limiters {
        limiter.wait_for(got).await;
    }
    bandwidth.record(got as u64);
    buffer.truncate(got);
    Ok(Some(Bytes::from(buffer)))
}

/// Shared by both [`FilePending`](BodyState::FilePending) (after it opens
/// and seeks the file) and [`FileOpen`](BodyState::FileOpen) (every poll
/// after the first) so there is exactly one place that decides "yield a
/// chunk, keep going" vs. "done, let `guard` drop" vs. "error, then done."
async fn read_next(
    mut file: tokio::fs::File,
    remaining: u64,
    rate_limiters: Vec<Arc<SessionRateLimiter>>,
    bandwidth: Arc<BandwidthMeter>,
    guard: SessionGuard,
) -> Option<(std::io::Result<Bytes>, BodyState)> {
    match read_file_chunk(&mut file, remaining, &rate_limiters, &bandwidth).await {
        Ok(Some(bytes)) => {
            let got = bytes.len() as u64;
            Some((
                Ok(bytes),
                BodyState::FileOpen {
                    file,
                    remaining: remaining - got,
                    rate_limiters,
                    bandwidth,
                    guard,
                },
            ))
        }
        // remaining == 0: body exhausted. Returning None here — rather than
        // yielding one last empty chunk — drops `guard` (owned by this
        // match arm's consumed state) right now, which is what actually
        // releases the transcode session.
        Ok(None) => None,
        Err(err) => Some((Err(err), BodyState::Finished { guard })),
    }
}

async fn next_body_chunk(state: BodyState) -> Option<(std::io::Result<Bytes>, BodyState)> {
    match state {
        BodyState::Bytes { bytes, guard } => Some((Ok(bytes), BodyState::Finished { guard })),
        BodyState::FilePending {
            path,
            offset,
            remaining,
            rate_limiters,
            bandwidth,
            guard,
        } => {
            let mut file = match tokio::fs::File::open(&path).await {
                Ok(file) => file,
                Err(err) => return Some((Err(err), BodyState::Finished { guard })),
            };
            if let Err(err) = file.seek(std::io::SeekFrom::Start(offset)).await {
                return Some((Err(err), BodyState::Finished { guard }));
            }
            read_next(file, remaining, rate_limiters, bandwidth, guard).await
        }
        BodyState::FileOpen {
            file,
            remaining,
            rate_limiters,
            bandwidth,
            guard,
        } => read_next(file, remaining, rate_limiters, bandwidth, guard).await,
        BodyState::Finished { .. } => None,
    }
}

/// Turns a [`Resolved`] into a chunked byte stream — the QUIC transport
/// ([`handle_stream`]) and any HTTP transport both consume this, so the
/// 64 KiB chunking, per-chunk rate limiting, bandwidth accounting, and
/// session-release-on-drop logic is written and tested exactly once rather
/// than reimplemented per transport (see [`SessionGuard`] for why the
/// release specifically must not depend on the stream finishing normally).
pub fn stream_body(
    resolved: Resolved,
    service: &Arc<MediaService>,
) -> impl Stream<Item = std::io::Result<Bytes>> + Send + 'static {
    let guard = SessionGuard {
        manager: Arc::clone(service.transcode_manager()),
        session_id: resolved.session_id,
    };
    let initial = match resolved.body {
        Body::Bytes(bytes) => BodyState::Bytes {
            bytes: Bytes::from(bytes),
            guard,
        },
        Body::File {
            path,
            offset,
            len,
            rate_limiters,
        } => BodyState::FilePending {
            path,
            offset,
            remaining: len,
            rate_limiters,
            bandwidth: Arc::clone(service.bandwidth_meter()),
            guard,
        },
    };
    stream::unfold(initial, next_body_chunk)
}

/// Serve one accepted bidi stream: read the request, resolve it, stream the
/// body out via [`stream_body`]. Takes `&Arc<MediaService>` rather than
/// `&MediaService` specifically so `stream_body`'s returned stream — which
/// must be `'static` since it can outlive this function's own stack frame if
/// ever spawned/boxed independently — can clone its own owned `Arc` instead
/// of borrowing one it doesn't have.
pub async fn handle_stream(
    service: &Arc<MediaService>,
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
    is_lan: bool,
) -> Result<(), P2pError> {
    let request = read_request(&mut recv).await?;
    let resolved = service.resolve_for_network(&request, is_lan).await;
    write_response_header(&mut send, &resolved.header).await?;
    let mut body = std::pin::pin!(stream_body(resolved, service));
    while let Some(chunk) = futures_util::StreamExt::next(&mut body).await {
        send.write_all(&chunk?).await?;
    }
    send.finish().ok();
    Ok(())
}

/// Serve every request stream an already-established connection sends,
/// spawning a task per stream, until the peer closes it. Split out from
/// [`accept_loop`] so a connection that arrived some other way — a punched
/// connection from `apps/server`'s `punch_connect`, say, rather than
/// `endpoint.accept()` — gets exactly the same per-stream serving behavior.
pub async fn serve_connection(connection: quinn::Connection, service: Arc<MediaService>) {
    let remote = connection.remote_address();
    let is_lan = is_lan_ip(remote.ip());
    tracing::info!(%remote, is_lan, "peer connected");
    // Loop ends when accept_bi errors, i.e. the connection closed.
    while let Ok((send, recv)) = connection.accept_bi().await {
        let service = Arc::clone(&service);
        tokio::spawn(async move {
            if let Err(err) = handle_stream(&service, send, recv, is_lan).await {
                tracing::debug!(error = %err, "stream failed");
            }
        });
    }
}

/// Shared LAN/private-address check — used to decide whether the shared
/// upload-bandwidth budget applies (see [`TranscodeManager::should_throttle`])
/// for QUIC peers here, and reused as-is by callers outside this crate
/// (`apps/server`'s LAN pairing and, later, its HTTP media surface) so there
/// is exactly one definition rather than an independent copy per transport.
pub fn is_lan_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_private() || ip.is_link_local() || ip.is_loopback(),
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip
                    .to_ipv4_mapped()
                    .is_some_and(|ip| ip.is_private() || ip.is_link_local() || ip.is_loopback())
        }
    }
}

/// Accept connections (already fingerprint-gated by the TLS layer) and spawn
/// a task per request stream.
pub async fn accept_loop(endpoint: quinn::Endpoint, service: Arc<MediaService>) {
    while let Some(incoming) = endpoint.accept().await {
        let service = Arc::clone(&service);
        tokio::spawn(async move {
            let connection = match incoming.await {
                Ok(connection) => connection,
                Err(err) => {
                    tracing::debug!(error = %err, "connection handshake failed");
                    return;
                }
            };
            serve_connection(connection, service).await;
        });
    }
}

#[cfg(test)]
mod network_tests {
    use super::is_lan_ip;

    #[test]
    fn identifies_local_and_internet_addresses() {
        assert!(is_lan_ip("192.168.1.20".parse().unwrap()));
        assert!(is_lan_ip("10.0.0.8".parse().unwrap()));
        assert!(is_lan_ip("fe80::1234".parse().unwrap()));
        assert!(is_lan_ip("fc00::1234".parse().unwrap()));
        assert!(!is_lan_ip("8.8.8.8".parse().unwrap()));
        assert!(!is_lan_ip("2606:4700:4700::1111".parse().unwrap()));
    }
}
