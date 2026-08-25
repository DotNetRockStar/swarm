//! Upload-budgeted playback planning and FFmpeg-backed HLS sessions.
//!
//! A session reserves its peak delivery rate before any URL is handed to a
//! client. The sum of reservations can never exceed the configured usable
//! upload budget, so independent players cannot each consume the full uplink.

use crate::probe::AudioStreamOption;
use crate::store::EntryRecord;
use rand::RngCore;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use swarm_core::peer::{MediaKind, PlaybackMode, PlaybackPlan, PlaybackPreferences};
use tokio::process::{Child, Command};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
const AUDIO_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const CLEANUP_INTERVAL: Duration = Duration::from_secs(30);
const PREVIEW_DURATION_SECS: u64 = 32;

#[derive(Debug, Clone)]
pub struct TranscodeConfig {
    pub enabled: bool,
    pub ffmpeg_path: PathBuf,
    pub session_dir: PathBuf,
    /// Total measured/configured upload before reserving bandwidth for the
    /// rest of the household.
    pub max_upload_bps: u64,
    /// Percentage excluded from streaming allocations. Clamped to 0..=90.
    pub reserve_percent: u8,
    pub max_sessions: usize,
    pub idle_timeout: Duration,
    pub segment_duration_secs: u32,
}

impl TranscodeConfig {
    pub fn usable_upload_bps(&self) -> u64 {
        let reserve = self.reserve_percent.min(90) as u64;
        self.max_upload_bps.saturating_mul(100 - reserve) / 100
    }

    pub fn disabled(session_dir: PathBuf) -> Self {
        Self {
            enabled: false,
            ffmpeg_path: PathBuf::from("ffmpeg"),
            session_dir,
            max_upload_bps: 10_000_000,
            reserve_percent: 30,
            max_sessions: 2,
            idle_timeout: Duration::from_secs(300),
            segment_duration_secs: 4,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TranscodeError {
    #[error("playback preferences are required")]
    MissingPreferences,
    #[error(
        "transcoding is disabled and this file cannot be direct-played within the upload budget"
    )]
    Disabled,
    #[error("server transcode capacity is full")]
    Capacity,
    #[error("not enough upload bandwidth is available for the lowest rendition")]
    Bandwidth,
    #[error("the client does not advertise HLS support")]
    Unsupported,
    #[error("could not prepare transcode workspace: {0}")]
    Workspace(#[source] std::io::Error),
    #[error("could not start ffmpeg: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("ffmpeg exited before producing a playlist: {0}")]
    Ffmpeg(String),
    #[error("ffmpeg did not produce a playlist within {} seconds", STARTUP_TIMEOUT.as_secs())]
    StartupTimeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rendition {
    name: &'static str,
    width: u32,
    height: u32,
    average_video_bps: u64,
    peak_video_bps: u64,
    audio_bps: u64,
}

#[derive(Debug, Clone, Copy)]
struct HlsReservation {
    reserved_bps: u64,
    enforce_budget: bool,
    budget_exempt: bool,
}

impl Rendition {
    fn peak_total(self) -> u64 {
        self.peak_video_bps + self.audio_bps
    }
}

/// One audio track ffmpeg will transcode into its own HLS rendition, shared
/// across every video rendition via a common `agroup` — see the "6 audio
/// tracks" follow-up on #55, where mapping only the single server-picked
/// track meant there was nothing for the client to actually switch between.
/// `name` is filesystem/URL-safe (used for both the output directory and the
/// HLS `NAME` attribute) and disambiguated when more than one track shares a
/// language; `language` is the raw ffprobe tag, kept separate so duplicate
/// tracks in the same language still report that same `LANGUAGE` attribute.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AudioMapEntry {
    source_map: String,
    name: String,
    language: Option<String>,
    is_default: bool,
}

fn sanitize_audio_tag(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .trim()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// Builds one [AudioMapEntry] per probed audio track, falling back to
/// ffmpeg's own `0:a:0` default-track selection when the probe found nothing
/// (ffprobe missing/failed, or a container ffprobe can't read). Always
/// returns at least one entry with `is_default` set, even on that fallback.
fn audio_map_entries(options: Vec<AudioStreamOption>) -> Vec<AudioMapEntry> {
    if options.is_empty() {
        return vec![AudioMapEntry {
            source_map: "0:a:0".to_string(),
            name: "und".to_string(),
            language: None,
            is_default: true,
        }];
    }
    let mut seen: HashMap<String, u32> = HashMap::new();
    let mut entries: Vec<AudioMapEntry> = options
        .into_iter()
        .map(|option| {
            let base = option
                .language
                .as_deref()
                .and_then(sanitize_audio_tag)
                .unwrap_or_else(|| "und".to_string());
            let count = seen.entry(base.clone()).or_insert(0);
            let name = if *count == 0 {
                base.clone()
            } else {
                format!("{base}{count}")
            };
            *count += 1;
            AudioMapEntry {
                source_map: format!("0:{}", option.index),
                name,
                language: option.language,
                is_default: option.is_preferred,
            }
        })
        .collect();
    if !entries.iter().any(|entry| entry.is_default) {
        if let Some(first) = entries.first_mut() {
            first.is_default = true;
        }
    }
    entries
}

const LADDER: [Rendition; 4] = [
    Rendition {
        name: "1080p",
        width: 1920,
        height: 1080,
        average_video_bps: 6_000_000,
        peak_video_bps: 8_000_000,
        audio_bps: 192_000,
    },
    Rendition {
        name: "720p",
        width: 1280,
        height: 720,
        average_video_bps: 3_000_000,
        peak_video_bps: 4_000_000,
        audio_bps: 160_000,
    },
    Rendition {
        name: "480p",
        width: 854,
        height: 480,
        average_video_bps: 1_400_000,
        peak_video_bps: 2_000_000,
        audio_bps: 128_000,
    },
    Rendition {
        name: "360p",
        width: 640,
        height: 360,
        average_video_bps: 700_000,
        peak_video_bps: 1_000_000,
        audio_bps: 96_000,
    },
];

/// Browse cards are only 347x195 dp. A single inexpensive rendition gets a
/// useful first frame to the TV faster than starting the full adaptive ladder,
/// while retaining enough detail for the enlarged card on a 4K display.
const PREVIEW_LADDER: [Rendition; 3] = [
    Rendition {
        name: "preview-540p",
        width: 960,
        height: 540,
        average_video_bps: 1_000_000,
        peak_video_bps: 1_400_000,
        audio_bps: 96_000,
    },
    Rendition {
        name: "preview-480p",
        width: 854,
        height: 480,
        average_video_bps: 850_000,
        peak_video_bps: 1_200_000,
        audio_bps: 96_000,
    },
    Rendition {
        name: "preview-360p",
        width: 640,
        height: 360,
        average_video_bps: 600_000,
        peak_video_bps: 900_000,
        audio_bps: 96_000,
    },
];

fn video_variants(
    source_height: u32,
    preferences: &PlaybackPreferences,
    client_limit: u64,
) -> Vec<Rendition> {
    let eligible = |rendition: &&Rendition| {
        rendition.width <= preferences.capabilities.max_width
            && rendition.height <= preferences.capabilities.max_height
            // The lowest HLS rung is a 640x360 *bounding box*. A source can
            // legitimately be shorter than 360 while still being wider than
            // 640 (real example: The Aviator AVI is 688x288); FFmpeg's
            // force_original_aspect_ratio=decrease scales that file down to
            // fit without enlarging/distorting it. Rejecting solely because
            // 288 < 360 left no rendition and surfaced a false bandwidth 429.
            && rendition.height <= source_height.max(360)
            && rendition.peak_total() <= client_limit
    };
    if preferences.preview {
        // Encoding multiple ABR rungs costs more CPU and delays the first
        // segment. One appropriately-sized preview stream is intentional.
        PREVIEW_LADDER
            .iter()
            .filter(eligible)
            .copied()
            .take(1)
            .collect()
    } else {
        LADDER.iter().filter(eligible).copied().collect()
    }
}

#[derive(Debug)]
enum SessionKind {
    Direct {
        entry_key: String,
    },
    Hls {
        directory: PathBuf,
        child: Option<Child>,
    },
}

#[derive(Debug)]
struct Session {
    kind: SessionKind,
    reserved_bps: u64,
    /// LAN sessions never consume the internet-uplink budget, including if
    /// the global preference is toggled while they are active.
    budget_exempt: bool,
    rate_limiter: Arc<SessionRateLimiter>,
    last_access: Instant,
    in_use: usize,
}

#[derive(Default)]
struct State {
    sessions: HashMap<String, Session>,
}

/**
 * Count ffmpeg processes that are actually still running. Completed HLS
 * sessions retain their generated files for playback, but no longer consume
 * a transcode slot; a child-less HLS session is still starting and does.
 */
fn active_hls_processes(state: &mut State) -> usize {
    state
        .sessions
        .values_mut()
        .map(|session| match &mut session.kind {
            SessionKind::Hls {
                child: Some(child), ..
            } => match child.try_wait() {
                Ok(Some(_)) => false,
                Ok(None) | Err(_) => true,
            },
            SessionKind::Hls { child: None, .. } => true,
            SessionKind::Direct { .. } => false,
        })
        .filter(|active| *active)
        .count()
}

pub struct SessionFile {
    pub path: PathBuf,
    pub rate_limiter: Arc<SessionRateLimiter>,
    pub session_id: String,
}

/// A shared per-session pacing clock. All concurrent HLS playlist, init, and
/// segment requests reserve time on this one clock, so opening several QUIC
/// streams cannot multiply a session's upload allocation.
#[derive(Debug)]
pub struct SessionRateLimiter {
    rate_bps: std::sync::atomic::AtomicU64,
    next_available: tokio::sync::Mutex<tokio::time::Instant>,
}

impl SessionRateLimiter {
    fn new(rate_bps: u64) -> Self {
        Self {
            rate_bps: std::sync::atomic::AtomicU64::new(rate_bps),
            next_available: tokio::sync::Mutex::new(tokio::time::Instant::now()),
        }
    }

    pub fn rate_bps(&self) -> u64 {
        self.rate_bps.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Live-updates the pacing rate — used when the auto-measured upload
    /// baseline (see `bandwidth` module in apps/server) changes, so an
    /// already-running session's throttle reflects it immediately rather
    /// than only affecting sessions negotiated after the update.
    pub fn set_rate_bps(&self, rate_bps: u64) {
        self.rate_bps
            .store(rate_bps, std::sync::atomic::Ordering::Relaxed);
    }

    pub async fn wait_for(&self, bytes: usize) {
        let rate_bps = self.rate_bps();
        if rate_bps == 0 || bytes == 0 {
            return;
        }
        let now = tokio::time::Instant::now();
        let send_at = {
            let mut next = self.next_available.lock().await;
            if *next < now {
                *next = now;
            }
            let send_at = *next;
            *next += Duration::from_secs_f64(bytes as f64 * 8.0 / rate_bps as f64);
            send_at
        };
        if send_at > now {
            tokio::time::sleep_until(send_at).await;
        }
    }
}

pub struct TranscodeManager {
    config: TranscodeConfig,
    /// Overrides `config.max_upload_bps` once a real measurement lands —
    /// see `set_max_upload_bps`. Starts equal to `config.max_upload_bps`,
    /// so behavior is unchanged until something actually updates it.
    max_upload_bps: std::sync::atomic::AtomicU64,
    upload_budget_enabled: std::sync::atomic::AtomicBool,
    state: Mutex<State>,
    global_rate_limiter: Arc<SessionRateLimiter>,
}

impl TranscodeManager {
    pub fn new(config: TranscodeConfig) -> Arc<Self> {
        cleanup_stale_session_dirs(&config.session_dir);
        let global_rate_limiter = Arc::new(SessionRateLimiter::new(config.usable_upload_bps()));
        let max_upload_bps = std::sync::atomic::AtomicU64::new(config.max_upload_bps);
        let manager = Arc::new(Self {
            config,
            max_upload_bps,
            upload_budget_enabled: std::sync::atomic::AtomicBool::new(true),
            state: Mutex::new(State::default()),
            global_rate_limiter,
        });
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let weak = Arc::downgrade(&manager);
            handle.spawn(async move { cleanup_loop(weak).await });
        }
        manager
    }

    pub fn config(&self) -> &TranscodeConfig {
        &self.config
    }

    /// The live usable streaming budget — `config.usable_upload_bps()`
    /// recomputed against whatever `max_upload_bps` currently holds
    /// (the configured/default value until a real measurement overrides
    /// it), not the static config value alone.
    pub fn usable_upload_bps(&self) -> u64 {
        let max = self
            .max_upload_bps
            .load(std::sync::atomic::Ordering::Relaxed);
        let reserve = self.config.reserve_percent.min(90) as u64;
        max.saturating_mul(100 - reserve) / 100
    }

    /// Called with a freshly measured real upload rate (see the
    /// `bandwidth` module) — updates both the admission-control budget
    /// (`usable_upload_bps`, gates *new* session negotiation) and the
    /// live global pacing rate (`global_rate_limiter`, throttles bytes
    /// actually being sent by sessions already in flight), so a change
    /// takes effect immediately rather than only for future sessions.
    pub fn set_max_upload_bps(&self, bps: u64) {
        self.max_upload_bps
            .store(bps, std::sync::atomic::Ordering::Relaxed);
        self.global_rate_limiter
            .set_rate_bps(if self.upload_budget_enabled() {
                self.usable_upload_bps()
            } else {
                0
            });
    }

    pub fn upload_budget_enabled(&self) -> bool {
        self.upload_budget_enabled
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Enables/disables both admission control and byte pacing. LAN
    /// connections are separately exempted by the request-serving layer.
    pub fn set_upload_budget_enabled(&self, enabled: bool) {
        self.upload_budget_enabled
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
        self.global_rate_limiter
            .set_rate_bps(if enabled { self.usable_upload_bps() } else { 0 });
        for session in self.state.lock().unwrap().sessions.values() {
            session
                .rate_limiter
                .set_rate_bps(if enabled { session.reserved_bps } else { 0 });
        }
    }

    pub fn should_throttle(&self, is_lan: bool) -> bool {
        self.upload_budget_enabled() && !is_lan
    }

    pub fn active_sessions(&self) -> usize {
        self.state.lock().unwrap().sessions.len()
    }

    pub fn reserved_bps(&self) -> u64 {
        self.state
            .lock()
            .unwrap()
            .sessions
            .values()
            .map(|session| session.reserved_bps)
            .sum()
    }

    pub fn global_rate_limiter(&self) -> Arc<SessionRateLimiter> {
        Arc::clone(&self.global_rate_limiter)
    }

    /// Select direct play when both the device and the shared uplink can
    /// safely carry the source. Otherwise reserve the highest eligible HLS
    /// rung and start an adaptive ladder containing it and every lower rung.
    pub async fn plan(
        &self,
        entry: &EntryRecord,
        media_path: &Path,
        preferences: &PlaybackPreferences,
        is_lan: bool,
    ) -> Result<PlaybackPlan, TranscodeError> {
        self.expire_idle();
        let enforce_budget = self.should_throttle(is_lan);
        let available = if enforce_budget {
            self.available_bps()?
        } else {
            u64::MAX
        };
        let client_limit = preferences.capabilities.max_bitrate.min(available);

        if preferences.prefer_direct && !preferences.preview {
            if let Some(source_peak) = direct_peak_bps(entry) {
                if source_peak <= client_limit && direct_compatible(entry, preferences) {
                    let id = self.reserve(
                        SessionKind::Direct {
                            entry_key: entry.entry_key.clone(),
                        },
                        source_peak,
                        enforce_budget,
                        is_lan,
                    )?;
                    return Ok(PlaybackPlan {
                        mode: PlaybackMode::Direct,
                        path: format!("/stream/{id}/media"),
                        max_bitrate: source_peak,
                        session_id: id,
                        lyrics: None,
                        subtitles: Vec::new(),
                    });
                }
            }
        }

        if !self.config.enabled {
            return Err(TranscodeError::Disabled);
        }
        if !preferences
            .capabilities
            .containers
            .iter()
            .any(|container| container.eq_ignore_ascii_case("hls"))
        {
            return Err(TranscodeError::Unsupported);
        }

        if entry.kind == MediaKind::Track {
            let audio_bps = 192_000u64.min(client_limit);
            if audio_bps < 96_000 {
                return Err(TranscodeError::Bandwidth);
            }
            return self
                .start_hls(
                    entry,
                    media_path,
                    preferences.start_position_secs,
                    &[],
                    HlsReservation {
                        reserved_bps: audio_bps,
                        enforce_budget,
                        budget_exempt: is_lan,
                    },
                    preferences.preview,
                )
                .await;
        }

        let source_height = entry
            .video
            .as_ref()
            .map(|video| video.height)
            .unwrap_or(preferences.capabilities.max_height);
        let variants = video_variants(source_height, preferences, client_limit);
        if variants.is_empty() {
            return Err(TranscodeError::Bandwidth);
        }
        // LADDER is high-to-low. The first entry is the reservation ceiling;
        // all remaining entries let ExoPlayer adapt downward without using
        // additional network bandwidth.
        let reserved_bps = variants[0].peak_total();
        self.start_hls(
            entry,
            media_path,
            preferences.start_position_secs,
            &variants,
            HlsReservation {
                reserved_bps,
                enforce_budget,
                budget_exempt: is_lan,
            },
            preferences.preview,
        )
        .await
    }

    pub fn open_direct(&self, session_id: &str) -> Option<(String, Arc<SessionRateLimiter>)> {
        let mut state = self.state.lock().unwrap();
        let session = state.sessions.get_mut(session_id)?;
        let SessionKind::Direct { entry_key } = &session.kind else {
            return None;
        };
        let entry_key = entry_key.clone();
        session.last_access = Instant::now();
        session.in_use += 1;
        Some((entry_key, Arc::clone(&session.rate_limiter)))
    }

    pub fn open_hls(&self, session_id: &str, relative_path: &str) -> Option<SessionFile> {
        if !safe_hls_path(relative_path) {
            return None;
        }
        let mut state = self.state.lock().unwrap();
        let session = state.sessions.get_mut(session_id)?;
        let SessionKind::Hls { directory, .. } = &session.kind else {
            return None;
        };
        let path = directory.join(relative_path);
        session.last_access = Instant::now();
        session.in_use += 1;
        Some(SessionFile {
            path,
            rate_limiter: Arc::clone(&session.rate_limiter),
            session_id: session_id.to_string(),
        })
    }

    pub fn finish_use(&self, session_id: &str) {
        if let Some(session) = self.state.lock().unwrap().sessions.get_mut(session_id) {
            session.in_use = session.in_use.saturating_sub(1);
            session.last_access = Instant::now();
        }
    }

    /// Client-initiated early release (`/stop/{id}`) — the player screen was
    /// torn down (back-press, or moving on to the next entry), so free this
    /// session's bandwidth reservation now instead of leaving it held for
    /// the full `idle_timeout`, which would otherwise reject a same-device
    /// replay attempt with `not enough upload bandwidth` for however long
    /// remained. Unconditional unlike `expire_idle` (no `in_use == 0`
    /// gate): the client is telling us it's done, not merely paused between
    /// segment requests.
    pub fn release(&self, session_id: &str) {
        self.remove_session(session_id);
    }

    // Bandwidth only — deliberately not gated on max_sessions here. This is
    // called once at the top of plan() before direct-vs-HLS is decided, and
    // max_sessions models concurrent *ffmpeg processes*, which a direct-play
    // session never spawns. Gating it here blocked plain direct-play-eligible
    // files (confirmed live: a music track that never needed a transcode at
    // all got a flat 429 "transcode capacity is full" because two earlier,
    // unrelated movie sessions hadn't hit their 5-minute idle expiry yet).
    fn available_bps(&self) -> Result<u64, TranscodeError> {
        let state = self.state.lock().unwrap();
        let reserved: u64 = state
            .sessions
            .values()
            .filter(|session| !session.budget_exempt)
            .map(|session| session.reserved_bps)
            .sum();
        Ok(self.usable_upload_bps().saturating_sub(reserved))
    }

    /// Only ever called for `SessionKind::Direct` (see `plan()`) — no
    /// max_sessions check for the same reason as `available_bps()` above;
    /// direct play is bandwidth-limited only, never process-limited.
    fn reserve(
        &self,
        kind: SessionKind,
        reserved_bps: u64,
        enforce_budget: bool,
        budget_exempt: bool,
    ) -> Result<String, TranscodeError> {
        let mut state = self.state.lock().unwrap();
        let already_reserved: u64 = state
            .sessions
            .values()
            .filter(|session| !session.budget_exempt)
            .map(|session| session.reserved_bps)
            .sum();
        if enforce_budget
            && already_reserved.saturating_add(reserved_bps) > self.usable_upload_bps()
        {
            return Err(TranscodeError::Bandwidth);
        }
        let id = session_id();
        state.sessions.insert(
            id.clone(),
            Session {
                kind,
                reserved_bps,
                budget_exempt,
                rate_limiter: Arc::new(SessionRateLimiter::new(reserved_bps)),
                last_access: Instant::now(),
                in_use: 0,
            },
        );
        Ok(id)
    }

    async fn start_hls(
        &self,
        entry: &EntryRecord,
        media_path: &Path,
        start_position_secs: u64,
        variants: &[Rendition],
        reservation: HlsReservation,
        preview: bool,
    ) -> Result<PlaybackPlan, TranscodeError> {
        let HlsReservation {
            reserved_bps,
            enforce_budget,
            budget_exempt,
        } = reservation;
        std::fs::create_dir_all(&self.config.session_dir).map_err(TranscodeError::Workspace)?;
        let id = session_id();
        let directory = self.config.session_dir.join(&id);
        std::fs::create_dir_all(&directory).map_err(TranscodeError::Workspace)?;
        if variants.is_empty() {
            std::fs::create_dir_all(directory.join("vaudio")).map_err(TranscodeError::Workspace)?;
        } else {
            for rendition in variants {
                std::fs::create_dir_all(directory.join(format!("v{}", rendition.name)))
                    .map_err(TranscodeError::Workspace)?;
            }
        }

        {
            let mut state = self.state.lock().unwrap();
            // Only count other Hls sessions here — a concurrently-open Direct
            // session holds no ffmpeg process and shouldn't spend a slot of
            // this specifically process/CPU-oriented limit.
            let transcode_sessions = active_hls_processes(&mut state);
            if transcode_sessions >= self.config.max_sessions.max(1) {
                let _ = std::fs::remove_dir_all(&directory);
                return Err(TranscodeError::Capacity);
            }
            let already_reserved: u64 = state
                .sessions
                .values()
                .filter(|session| !session.budget_exempt)
                .map(|session| session.reserved_bps)
                .sum();
            if enforce_budget
                && already_reserved.saturating_add(reserved_bps) > self.usable_upload_bps()
            {
                let _ = std::fs::remove_dir_all(&directory);
                return Err(TranscodeError::Bandwidth);
            }
            state.sessions.insert(
                id.clone(),
                Session {
                    kind: SessionKind::Hls {
                        directory: directory.clone(),
                        child: None,
                    },
                    reserved_bps,
                    budget_exempt,
                    rate_limiter: Arc::new(SessionRateLimiter::new(reserved_bps)),
                    last_access: Instant::now(),
                    in_use: 0,
                },
            );
        }

        let result = self
            .spawn_ffmpeg(
                entry,
                media_path,
                start_position_secs,
                variants,
                reserved_bps,
                &directory,
                preview,
            )
            .await;
        let child = match result {
            Ok(child) => child,
            Err(error) => {
                self.remove_session(&id);
                return Err(error);
            }
        };
        if let Some(Session {
            kind: SessionKind::Hls { child: slot, .. },
            ..
        }) = self.state.lock().unwrap().sessions.get_mut(&id)
        {
            *slot = Some(child);
        }

        Ok(PlaybackPlan {
            mode: PlaybackMode::Hls,
            path: format!("/hls/{id}/master.m3u8"),
            max_bitrate: reserved_bps,
            session_id: id,
            lyrics: None,
            subtitles: Vec::new(),
        })
    }

    async fn spawn_ffmpeg(
        &self,
        entry: &EntryRecord,
        media_path: &Path,
        start_position_secs: u64,
        variants: &[Rendition],
        reserved_bps: u64,
        directory: &Path,
        preview: bool,
    ) -> Result<Child, TranscodeError> {
        let log_path = directory.join("ffmpeg.log");
        let log = std::fs::File::create(&log_path).map_err(TranscodeError::Workspace)?;
        let mut command = Command::new(&self.config.ffmpeg_path);
        command
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("warning")
            .arg("-nostdin")
            .arg("-y");
        if start_position_secs > 0 {
            command.arg("-ss").arg(start_position_secs.to_string());
        }
        command.arg("-i").arg(media_path);
        if preview {
            // The client displays 30 seconds. Stop the encoder shortly after
            // that window instead of racing through the remainder of a movie.
            command.arg("-t").arg(PREVIEW_DURATION_SECS.to_string());
        }

        let has_audio = entry.audio.is_some();
        // Audio language is routing metadata, not part of the codec summary
        // retained at scan time. Resolve every embedded audio stream (not
        // just one) when an HLS session is created, so both normal
        // transcodes and previews can offer the viewer every track the
        // container actually has — mapping only a single server-picked
        // track (the historic behavior) left nothing for the pause/playback
        // screen to switch between even when the source had six (#55). A
        // failed/missing ffprobe, or a Track (music) entry, safely retains
        // the original single 0:a:0-mapped stream. Bound this metadata read
        // tightly because a slow network share must not hold a hover
        // preview in negotiation.
        let audio_tracks: Vec<AudioMapEntry> = if !has_audio {
            Vec::new()
        } else if entry.kind == MediaKind::Track {
            vec![AudioMapEntry {
                source_map: "0:a:0".to_string(),
                name: "audio".to_string(),
                language: None,
                is_default: true,
            }]
        } else {
            let options = tokio::time::timeout(
                AUDIO_PROBE_TIMEOUT,
                crate::probe::list_audio_streams(&self.config.ffmpeg_path, media_path),
            )
            .await
            .ok()
            .unwrap_or_default();
            audio_map_entries(options)
        };
        let output_pattern = directory.join("v%v/index.m3u8");
        let segment_pattern = directory.join("v%v/segment_%06d.m4s");

        if variants.is_empty() {
            let audio = audio_tracks.first().cloned().unwrap_or(AudioMapEntry {
                source_map: "0:a:0".to_string(),
                name: "audio".to_string(),
                language: None,
                is_default: true,
            });
            command
                .arg("-vn")
                .arg("-map")
                .arg(&audio.source_map)
                .arg("-c:a:0")
                .arg("aac")
                .arg("-b:a:0")
                .arg(reserved_bps.to_string())
                .arg("-ac:a:0")
                .arg("2")
                .arg("-ar:a:0")
                .arg("48000");
        } else {
            // Each track's own subdirectory must exist before ffmpeg starts
            // writing into it, same as the per-rendition video directories
            // created by the caller (start_hls) — ffmpeg's hls muxer does
            // not create intermediate directories itself.
            for audio in &audio_tracks {
                std::fs::create_dir_all(directory.join(format!("v{}", audio.name)))
                    .map_err(TranscodeError::Workspace)?;
            }
            let split_labels = (0..variants.len())
                .map(|index| format!("[split{index}]"))
                .collect::<String>();
            let mut filter = format!("[0:v:0]split={}{};", variants.len(), split_labels);
            for (index, rendition) in variants.iter().enumerate() {
                filter.push_str(&format!(
                    "[split{index}]scale=w={}:h={}:force_original_aspect_ratio=decrease:force_divisible_by=2[v{index}];",
                    rendition.width, rendition.height
                ));
            }
            filter.pop();
            command.arg("-filter_complex").arg(filter);
            for (index, rendition) in variants.iter().enumerate() {
                command.arg("-map").arg(format!("[v{index}]"));
                command
                    .arg(format!("-c:v:{index}"))
                    .arg("libx264")
                    .arg(format!("-preset:v:{index}"))
                    .arg(if preview { "ultrafast" } else { "veryfast" })
                    .arg(format!("-profile:v:{index}"))
                    .arg("high")
                    .arg(format!("-level:v:{index}"))
                    .arg("4.1")
                    .arg(format!("-pix_fmt:v:{index}"))
                    .arg("yuv420p");
                if preview {
                    command.arg(format!("-tune:v:{index}")).arg("zerolatency");
                }
                command
                    .arg(format!("-b:v:{index}"))
                    .arg(rendition.average_video_bps.to_string())
                    .arg(format!("-maxrate:v:{index}"))
                    .arg(rendition.peak_video_bps.to_string())
                    .arg(format!("-bufsize:v:{index}"))
                    .arg((rendition.peak_video_bps * 2).to_string())
                    .arg(format!("-sc_threshold:v:{index}"))
                    .arg("0")
                    .arg(format!("-force_key_frames:v:{index}"))
                    .arg(if preview {
                        "expr:gte(t,n_forced*1)"
                    } else {
                        "expr:gte(t,n_forced*2)"
                    });
            }
            // Every embedded audio track is transcoded exactly once and
            // shared across all video renditions through a common `agroup`
            // below, instead of the old one-audio-encode-per-video-rendition
            // scheme — that duplication only ever carried the single
            // server-picked track anyway, so a viewer could never choose a
            // different one (#55). The top rung's audio bitrate covers every
            // track since HLS only ever streams whichever one is currently
            // selected, never all of them at once.
            if has_audio {
                let audio_bps = variants[0].audio_bps;
                for (index, audio) in audio_tracks.iter().enumerate() {
                    command
                        .arg("-map")
                        .arg(&audio.source_map)
                        .arg(format!("-c:a:{index}"))
                        .arg("aac")
                        .arg(format!("-b:a:{index}"))
                        .arg(audio_bps.to_string())
                        .arg(format!("-ac:a:{index}"))
                        .arg("2")
                        .arg(format!("-ar:a:{index}"))
                        .arg("48000");
                }
            }
        }

        let stream_map = if variants.is_empty() {
            "a:0,name:audio".to_string()
        } else {
            let mut entries: Vec<String> = variants
                .iter()
                .enumerate()
                .map(|(index, rendition)| {
                    if has_audio {
                        format!("v:{index},agroup:aud,name:{}", rendition.name)
                    } else {
                        format!("v:{index},name:{}", rendition.name)
                    }
                })
                .collect();
            for (index, audio) in audio_tracks.iter().enumerate() {
                let mut audio_entry = format!("a:{index},agroup:aud,name:{}", audio.name);
                if let Some(language) = audio.language.as_deref().and_then(sanitize_audio_tag) {
                    audio_entry.push_str(&format!(",language:{language}"));
                }
                audio_entry.push_str(if audio.is_default {
                    ",default:yes"
                } else {
                    ",default:no"
                });
                entries.push(audio_entry);
            }
            entries.join(" ")
        };

        command
            .arg("-f")
            .arg("hls")
            .arg("-hls_time")
            .arg(if preview {
                "1".to_string()
            } else {
                self.config.segment_duration_secs.max(2).to_string()
            })
            .arg("-hls_list_size")
            .arg("0")
            .arg("-hls_playlist_type")
            .arg("event")
            .arg("-hls_segment_type")
            .arg("fmp4")
            .arg("-hls_fmp4_init_filename")
            .arg("init.mp4")
            .arg("-hls_flags")
            .arg("independent_segments+temp_file")
            .arg("-master_pl_name")
            .arg("master.m3u8")
            .arg("-var_stream_map")
            .arg(stream_map)
            .arg("-hls_segment_filename")
            .arg(segment_pattern)
            .arg(output_pattern)
            .kill_on_drop(true)
            .stdout(Stdio::null())
            .stderr(Stdio::from(log));

        let mut child = command.spawn().map_err(TranscodeError::Spawn)?;
        let master = directory.join("master.m3u8");
        let started = Instant::now();
        loop {
            if master.metadata().is_ok_and(|metadata| metadata.len() > 0) {
                return Ok(child);
            }
            if let Some(status) = child.try_wait().map_err(TranscodeError::Spawn)? {
                let detail = std::fs::read_to_string(&log_path)
                    .unwrap_or_else(|_| format!("exit status {status}"));
                return Err(TranscodeError::Ffmpeg(detail.trim().to_string()));
            }
            if started.elapsed() >= STARTUP_TIMEOUT {
                let _ = child.start_kill();
                return Err(TranscodeError::StartupTimeout);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    fn expire_idle(&self) {
        let now = Instant::now();
        let expired: Vec<String> = {
            let state = self.state.lock().unwrap();
            state
                .sessions
                .iter()
                .filter(|(_, session)| {
                    session.in_use == 0
                        && now.duration_since(session.last_access) >= self.config.idle_timeout
                })
                .map(|(id, _)| id.clone())
                .collect()
        };
        for id in expired {
            self.remove_session(&id);
        }
    }

    fn remove_session(&self, id: &str) {
        let removed = self.state.lock().unwrap().sessions.remove(id);
        if let Some(Session {
            kind:
                SessionKind::Hls {
                    directory,
                    mut child,
                },
            ..
        }) = removed
        {
            if let Some(child) = child.as_mut() {
                let _ = child.start_kill();
            }
            let _ = std::fs::remove_dir_all(directory);
        }
    }
}

impl Drop for TranscodeManager {
    fn drop(&mut self) {
        let sessions = std::mem::take(&mut self.state.lock().unwrap().sessions);
        for (_, session) in sessions {
            if let SessionKind::Hls {
                directory,
                mut child,
            } = session.kind
            {
                if let Some(child) = child.as_mut() {
                    let _ = child.start_kill();
                }
                let _ = std::fs::remove_dir_all(directory);
            }
        }
    }
}

async fn cleanup_loop(manager: Weak<TranscodeManager>) {
    let mut interval = tokio::time::interval(CLEANUP_INTERVAL);
    interval.tick().await;
    loop {
        interval.tick().await;
        let Some(manager) = manager.upgrade() else {
            return;
        };
        manager.expire_idle();
    }
}

fn cleanup_stale_session_dirs(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.len() == 32
            && name.chars().all(|ch| ch.is_ascii_hexdigit())
            && entry.path().is_dir()
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn direct_peak_bps(entry: &EntryRecord) -> Option<u64> {
    let duration = entry.duration_secs.filter(|duration| *duration > 0.0)?;
    let measured_average = (entry.size as f64 * 8.0 / duration) as u64;
    let stream_sum = entry
        .video
        .as_ref()
        .and_then(|video| video.bitrate)
        .unwrap_or(0)
        + entry
            .audio
            .as_ref()
            .and_then(|audio| audio.bitrate)
            .unwrap_or(0);
    Some(measured_average.max(stream_sum).saturating_mul(5) / 4)
}

fn direct_compatible(entry: &EntryRecord, preferences: &PlaybackPreferences) -> bool {
    let capabilities = &preferences.capabilities;
    let extension = entry
        .relative_path
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let container = match extension.as_str() {
        "m4v" | "m4a" | "mov" => "mp4",
        other => other,
    };
    if !capabilities
        .containers
        .iter()
        .any(|supported| supported.eq_ignore_ascii_case(container))
    {
        return false;
    }
    if entry.kind != MediaKind::Track {
        if let Some(video) = &entry.video {
            if video.width > capabilities.max_width || video.height > capabilities.max_height {
                return false;
            }
            if !capabilities
                .video_codecs
                .iter()
                .any(|supported| codec_matches(supported, &video.codec))
            {
                return false;
            }
        }
    }
    if let Some(audio) = &entry.audio {
        if !capabilities
            .audio_codecs
            .iter()
            .any(|supported| codec_matches(supported, &audio.codec))
        {
            return false;
        }
    }
    true
}

fn codec_matches(supported: &str, actual: &str) -> bool {
    supported
        .split([':', '@'])
        .next()
        .unwrap_or(supported)
        .eq_ignore_ascii_case(actual)
}

fn session_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn safe_hls_path(path: &str) -> bool {
    let extension_allowed = matches!(path.rsplit('.').next(), Some("m3u8" | "m4s" | "mp4"));
    extension_allowed
        && !path.is_empty()
        && !path.starts_with('/')
        && path.split('/').all(|segment| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && segment
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
        })
}

pub fn hls_content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "m3u8" => "application/vnd.apple.mpegurl",
        "m4s" => "video/iso.segment",
        "mp4" => "video/mp4",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarm_core::capability::CapabilityProfile;
    use swarm_core::peer::{AudioStreamInfo, MediaKind, VideoStreamInfo};

    fn entry() -> EntryRecord {
        EntryRecord {
            entry_key: "0123456789abcdef01234567".into(),
            relative_path: "movies/example.mkv".into(),
            kind: MediaKind::Movie,
            title: "Example".into(),
            size: 5_400_000_000,
            modified_time: 0,
            fingerprint: "fp".into(),
            artist: None,
            album: None,
            track_number: None,
            show_title: None,
            season: None,
            episode: None,
            year: None,
            duration_secs: Some(7_200.0),
            video: Some(VideoStreamInfo {
                codec: "h264".into(),
                width: 1920,
                height: 1080,
                level: Some("4.1".into()),
                bitrate: Some(5_800_000),
            }),
            audio: Some(AudioStreamInfo {
                codec: "aac".into(),
                channels: 2,
                bitrate: Some(192_000),
            }),
            scraped_title: None,
            episode_title: None,
            genres: vec![],
            artwork_version: 0,
            cast: vec![],
            overview: None,
            rating: None,
            community_rating: None,
            community_rating_votes: None,
        }
    }

    fn preferences() -> PlaybackPreferences {
        PlaybackPreferences {
            capabilities: CapabilityProfile::fire_tv_baseline(),
            start_position_secs: 0,
            prefer_direct: true,
            preview: false,
        }
    }

    #[test]
    fn upload_reserve_is_removed_before_allocating() {
        let config = TranscodeConfig {
            enabled: true,
            ffmpeg_path: "ffmpeg".into(),
            session_dir: std::env::temp_dir().join("swarm-transcode-test"),
            max_upload_bps: 10_000_000,
            reserve_percent: 30,
            max_sessions: 2,
            idle_timeout: Duration::from_secs(300),
            segment_duration_secs: 4,
        };
        assert_eq!(config.usable_upload_bps(), 7_000_000);
    }

    #[test]
    fn upload_budget_can_be_disabled_and_never_applies_on_lan() {
        let manager = TranscodeManager::new(TranscodeConfig::disabled(
            std::env::temp_dir().join("swarm-budget-toggle-test"),
        ));
        assert!(manager.should_throttle(false));
        assert!(!manager.should_throttle(true));
        manager.set_upload_budget_enabled(false);
        assert!(!manager.should_throttle(false));
        assert_eq!(manager.global_rate_limiter().rate_bps(), 0);
    }

    #[tokio::test]
    async fn completed_hls_jobs_no_longer_consume_transcode_capacity() {
        let mut completed_child = Command::new("sh").args(["-c", "exit 0"]).spawn().unwrap();
        assert!(completed_child.wait().await.unwrap().success());

        let session = |kind| Session {
            kind,
            reserved_bps: 128_000,
            budget_exempt: true,
            rate_limiter: Arc::new(SessionRateLimiter::new(0)),
            last_access: Instant::now(),
            in_use: 0,
        };
        let mut state = State::default();
        state.sessions.insert(
            "finished".into(),
            session(SessionKind::Hls {
                directory: PathBuf::from("finished"),
                child: Some(completed_child),
            }),
        );
        assert_eq!(active_hls_processes(&mut state), 0);

        // `None` is the short interval after a session is reserved but
        // before spawn_ffmpeg installs its child handle; it must still hold
        // a slot so simultaneous negotiations cannot exceed the limit.
        state.sessions.insert(
            "starting".into(),
            session(SessionKind::Hls {
                directory: PathBuf::from("starting"),
                child: None,
            }),
        );
        state.sessions.insert(
            "direct".into(),
            session(SessionKind::Direct {
                entry_key: "track".into(),
            }),
        );
        assert_eq!(active_hls_processes(&mut state), 1);
    }

    #[test]
    fn direct_play_requires_container_codec_and_budget_compatibility() {
        let mut compatible = entry();
        compatible.relative_path = "movies/example.mp4".into();
        assert!(direct_compatible(&compatible, &preferences()));
        assert!(direct_peak_bps(&compatible).unwrap() > 6_000_000);

        let incompatible = entry();
        assert!(!direct_compatible(&incompatible, &preferences()));
    }

    #[test]
    fn preview_uses_one_lightweight_rendition() {
        let mut prefs = preferences();
        prefs.preview = true;
        prefs.prefer_direct = false;

        let variants = video_variants(1080, &prefs, u64::MAX);
        assert_eq!(variants.len(), 1);
        assert_eq!(variants[0].name, "preview-540p");
        assert_eq!(variants[0].height, 540);
        assert_eq!(variants[0].peak_total(), 1_496_000);

        prefs.capabilities.max_height = 480;
        let variants = video_variants(1080, &prefs, u64::MAX);
        assert_eq!(variants.len(), 1);
        assert_eq!(variants[0].name, "preview-480p");
    }

    #[test]
    fn low_height_widescreen_video_still_gets_the_lowest_hls_rendition() {
        let prefs = preferences();

        let variants = video_variants(288, &prefs, u64::MAX);

        assert_eq!(variants.len(), 1);
        assert_eq!(variants[0].name, "360p");
    }

    #[test]
    fn hls_paths_cannot_escape_the_session_directory() {
        assert!(safe_hls_path("v0/segment_000001.m4s"));
        assert!(!safe_hls_path("../secret"));
        assert!(!safe_hls_path("v0//segment.m4s"));
        assert!(!safe_hls_path("/absolute"));
        assert!(!safe_hls_path("ffmpeg.log"));
    }

    #[tokio::test]
    async fn reservations_share_one_upload_budget() {
        let root = std::env::temp_dir().join(format!("swarm-budget-test-{}", session_id()));
        let manager = TranscodeManager::new(TranscodeConfig {
            enabled: false,
            ffmpeg_path: "ffmpeg".into(),
            session_dir: root.clone(),
            max_upload_bps: 10_000_000,
            reserve_percent: 30,
            max_sessions: 3,
            idle_timeout: Duration::from_secs(300),
            segment_duration_secs: 4,
        });
        assert_eq!(manager.global_rate_limiter().rate_bps(), 7_000_000);
        let first = manager
            .reserve(
                SessionKind::Direct {
                    entry_key: "a".into(),
                },
                4_160_000,
                true,
                false,
            )
            .unwrap();
        let second = manager
            .reserve(
                SessionKind::Direct {
                    entry_key: "b".into(),
                },
                2_128_000,
                true,
                false,
            )
            .unwrap();
        assert_eq!(manager.reserved_bps(), 6_288_000);
        assert!(matches!(
            manager.reserve(
                SessionKind::Direct {
                    entry_key: "c".into()
                },
                1_096_000,
                true,
                false,
            ),
            Err(TranscodeError::Bandwidth)
        ));
        assert_eq!(manager.open_direct(&first).unwrap().1.rate_bps(), 4_160_000);
        manager.finish_use(&first);
        assert_eq!(
            manager.open_direct(&second).unwrap().1.rate_bps(),
            2_128_000
        );
        manager.finish_use(&second);
        drop(manager);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn ffmpeg_hls_pipeline_smoke_test_when_ffmpeg_is_available() {
        if Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .is_err()
        {
            return;
        }
        let root = std::env::temp_dir().join(format!("swarm-hls-smoke-{}", session_id()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("source.mp4");
        let generated = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=1280x720:rate=30:duration=2",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=2",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
                "-y",
            ])
            .arg(&source)
            .status()
            .await
            .unwrap();
        if !generated.success() {
            let _ = std::fs::remove_dir_all(&root);
            return;
        }

        let config = TranscodeConfig {
            enabled: true,
            ffmpeg_path: "ffmpeg".into(),
            session_dir: root.join("sessions"),
            max_upload_bps: 10_000_000,
            reserve_percent: 30,
            max_sessions: 1,
            idle_timeout: Duration::from_secs(300),
            segment_duration_secs: 4,
        };
        let manager = TranscodeManager::new(config);
        let mut source_entry = entry();
        source_entry.relative_path = "source.mp4".into();
        source_entry.size = source.metadata().unwrap().len();
        source_entry.duration_secs = Some(2.0);
        source_entry.video.as_mut().unwrap().width = 1280;
        source_entry.video.as_mut().unwrap().height = 720;
        source_entry.video.as_mut().unwrap().bitrate = Some(3_000_000);
        source_entry.audio.as_mut().unwrap().bitrate = Some(96_000);
        let mut prefs = preferences();
        prefs.prefer_direct = false;

        let plan = manager
            .plan(&source_entry, &source, &prefs, false)
            .await
            .unwrap();
        assert_eq!(plan.mode, PlaybackMode::Hls);
        let relative = plan.path.splitn(4, '/').nth(3).unwrap();
        let session = plan.path.split('/').nth(2).unwrap();
        let file = manager.open_hls(session, relative).unwrap();
        let master = std::fs::read_to_string(file.path).unwrap();
        assert!(master.contains("#EXTM3U"));
        assert!(master.contains("index.m3u8"));
        assert_eq!(master.matches("#EXT-X-STREAM-INF").count(), 3);
        manager.finish_use(session);

        manager.release(session);
        prefs.preview = true;
        let preview_plan = manager
            .plan(&source_entry, &source, &prefs, false)
            .await
            .unwrap();
        assert_eq!(preview_plan.mode, PlaybackMode::Hls);
        assert_eq!(preview_plan.max_bitrate, 1_496_000);
        let preview_relative = preview_plan.path.splitn(4, '/').nth(3).unwrap();
        let preview_session = preview_plan.path.split('/').nth(2).unwrap();
        let preview_file = manager.open_hls(preview_session, preview_relative).unwrap();
        let preview_master = std::fs::read_to_string(preview_file.path).unwrap();
        assert!(preview_master.contains("preview-540p/index.m3u8"));
        assert_eq!(preview_master.matches("#EXT-X-STREAM-INF").count(), 1);
        manager.finish_use(preview_session);
        drop(manager);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Regression coverage for the #55 follow-up ("6 audio tracks and 4
    /// subtitle tracks, but the player only offers Auto/Unknown") — the
    /// single-audio-stream smoke test above can't catch a `var_stream_map`
    /// syntax mistake or a missing per-track output directory, since ffmpeg
    /// is happy either way when there is only one track to map. This drives
    /// spawn_ffmpeg with a source that actually has two, so a real ffmpeg
    /// binary has to accept the multi-track command and produce a
    /// selectable rendition per language.
    #[tokio::test]
    async fn ffmpeg_hls_master_playlist_exposes_every_embedded_audio_track() {
        if Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .is_err()
        {
            return;
        }
        let root = std::env::temp_dir().join(format!("swarm-hls-multi-audio-{}", session_id()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("source.mp4");
        let generated = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=640x360:rate=30:duration=2",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=2",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=220:duration=2",
                "-map",
                "0:v",
                "-map",
                "1:a",
                "-map",
                "2:a",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-metadata:s:a:0",
                "language=eng",
                "-metadata:s:a:1",
                "language=jpn",
                "-shortest",
                "-y",
            ])
            .arg(&source)
            .status()
            .await
            .unwrap();
        if !generated.success() {
            let _ = std::fs::remove_dir_all(&root);
            return;
        }

        let config = TranscodeConfig {
            enabled: true,
            ffmpeg_path: "ffmpeg".into(),
            session_dir: root.join("sessions"),
            max_upload_bps: 10_000_000,
            reserve_percent: 30,
            max_sessions: 1,
            idle_timeout: Duration::from_secs(300),
            segment_duration_secs: 4,
        };
        let manager = TranscodeManager::new(config);
        let mut source_entry = entry();
        source_entry.relative_path = "source.mp4".into();
        source_entry.size = source.metadata().unwrap().len();
        source_entry.duration_secs = Some(2.0);
        source_entry.video.as_mut().unwrap().width = 640;
        source_entry.video.as_mut().unwrap().height = 360;
        source_entry.video.as_mut().unwrap().bitrate = Some(900_000);
        source_entry.audio.as_mut().unwrap().bitrate = Some(96_000);
        let mut prefs = preferences();
        prefs.prefer_direct = false;

        let plan = manager
            .plan(&source_entry, &source, &prefs, false)
            .await
            .unwrap();
        assert_eq!(plan.mode, PlaybackMode::Hls);
        let relative = plan.path.splitn(4, '/').nth(3).unwrap();
        let session = plan.path.split('/').nth(2).unwrap();
        let file = manager.open_hls(session, relative).unwrap();
        let master = std::fs::read_to_string(&file.path).unwrap();

        assert_eq!(master.matches("#EXT-X-MEDIA:TYPE=AUDIO").count(), 2);
        let eng_line = master
            .lines()
            .find(|line| line.contains("LANGUAGE=\"eng\""))
            .expect("English audio rendition missing from master playlist");
        assert!(
            eng_line.contains("DEFAULT=YES"),
            "English should win the same preferred-track tie-break used for direct play/single-track HLS: {eng_line}"
        );
        let jpn_line = master
            .lines()
            .find(|line| line.contains("LANGUAGE=\"jpn\""))
            .expect("Japanese audio rendition missing from master playlist");
        assert!(jpn_line.contains("DEFAULT=NO"), "{jpn_line}");

        // Prove ffmpeg actually produced a real, independently selectable
        // rendition for each track, not just a playlist that references one.
        let session_dir = root.join("sessions").join(session);
        assert!(session_dir.join("veng").join("index.m3u8").is_file());
        assert!(session_dir.join("vjpn").join("index.m3u8").is_file());

        manager.finish_use(session);
        drop(manager);
        let _ = std::fs::remove_dir_all(&root);
    }
}
