//! Durable local subtitle generation. Jobs and completed segments live in
//! the library database, so hiding the UI keeps work running and a real
//! process restart resumes at the first unfinished ten-minute segment.

use futures_util::StreamExt;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use swarm_media::roots::SharedRootResolver;
use swarm_media::store::{Library, SubtitleRecord, TranscriptionJob};
use swarm_media::transcode::TranscodeManager;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Notify, RwLock};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

const MODEL_NAME: &str = "base.en";
const MODEL_FILENAME: &str = "ggml-base.en.bin";
const MODEL_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin";
// Official whisper.cpp model digest is SHA-1. The download is still written
// to a .part file and accepted only after that published digest matches.
const MODEL_SHA1: &str = "137c40403d78fd54d454da0f9bd998f78703390c";
const SEGMENT_DURATION_SECS: u64 = 600;
const MEDIA_ROOT_UNAVAILABLE_PREFIX: &str = "media root unavailable:";

#[derive(Debug, Clone, serde::Serialize)]
pub struct TranscriptionStatus {
    pub enabled: bool,
    pub phase: String,
    pub message: String,
    pub model_name: String,
    pub model_installed: bool,
    pub downloaded_bytes: u64,
    pub download_total_bytes: u64,
    pub queued: u64,
    pub completed: u64,
    pub failed: u64,
    pub total_segments: u64,
    pub completed_segments: u64,
    pub current_title: Option<String>,
    pub current_segment: u32,
    pub current_total_segments: u32,
    pub current_segment_progress: u32,
}

#[derive(Debug, Clone)]
struct RuntimeStatus {
    phase: String,
    message: String,
    downloaded_bytes: u64,
    download_total_bytes: u64,
    current_title: Option<String>,
    current_segment: u32,
    current_total_segments: u32,
}

impl Default for RuntimeStatus {
    fn default() -> Self {
        Self {
            phase: "disabled".into(),
            message: "Local subtitle generation is disabled.".into(),
            downloaded_bytes: 0,
            download_total_bytes: 0,
            current_title: None,
            current_segment: 0,
            current_total_segments: 0,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Cue {
    start_ms: u64,
    end_ms: u64,
    text: String,
}

pub struct TranscriptionManager {
    library: Arc<Library>,
    roots: SharedRootResolver,
    transcodes: Arc<TranscodeManager>,
    enabled: Arc<AtomicBool>,
    pause_while_streaming: Arc<AtomicBool>,
    skip_if_subtitles_exist: Arc<AtomicBool>,
    scan_active: Arc<AtomicBool>,
    segment_progress: Arc<AtomicU32>,
    notify: Notify,
    runtime: RwLock<RuntimeStatus>,
    model_dir: PathBuf,
    ffmpeg_path: PathBuf,
}

impl TranscriptionManager {
    pub async fn start(
        library: Arc<Library>,
        roots: SharedRootResolver,
        transcodes: Arc<TranscodeManager>,
        scan_active: Arc<AtomicBool>,
        data_dir: &Path,
        ffmpeg_path: PathBuf,
    ) -> Result<Arc<Self>, sqlx::Error> {
        library.recover_interrupted_transcriptions().await?;
        let manager = Arc::new(Self {
            library,
            roots,
            transcodes,
            enabled: Arc::new(AtomicBool::new(false)),
            pause_while_streaming: Arc::new(AtomicBool::new(true)),
            skip_if_subtitles_exist: Arc::new(AtomicBool::new(false)),
            scan_active,
            segment_progress: Arc::new(AtomicU32::new(0)),
            notify: Notify::new(),
            runtime: RwLock::new(RuntimeStatus::default()),
            model_dir: data_dir.join("models").join("whisper"),
            ffmpeg_path,
        });
        let worker = Arc::clone(&manager);
        tokio::spawn(async move { worker.run().await });
        Ok(manager)
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
        self.notify.notify_waiters();
    }

    pub fn set_pause_while_streaming(&self, enabled: bool) {
        self.pause_while_streaming.store(enabled, Ordering::Release);
        self.notify.notify_waiters();
    }

    pub fn set_skip_if_subtitles_exist(&self, enabled: bool) {
        self.skip_if_subtitles_exist
            .store(enabled, Ordering::Release);
    }

    /// Force one movie/episode into the queue regardless of the bulk
    /// skip-if-exists preference — the "targeted creation" entry point.
    pub async fn enqueue_entry(&self, entry_key: &str) -> Result<(), String> {
        let queued = self
            .library
            .enqueue_transcription_for_entry(entry_key, MODEL_NAME, "en", SEGMENT_DURATION_SECS)
            .await
            .map_err(|error| error.to_string())?;
        if !queued {
            return Err(
                "This item isn't eligible for subtitle generation (movies and TV episodes with an audio track only)."
                    .into(),
            );
        }
        self.notify.notify_waiters();
        Ok(())
    }

    fn should_pause_for_streaming(&self) -> bool {
        self.pause_while_streaming.load(Ordering::Acquire) && self.transcodes.active_sessions() > 0
    }

    fn should_pause_for_scan(&self) -> bool {
        self.scan_active.load(Ordering::Acquire)
    }

    pub async fn status(&self) -> Result<TranscriptionStatus, sqlx::Error> {
        let queue = self.library.transcription_queue_status().await?;
        let runtime = self.runtime.read().await.clone();
        Ok(TranscriptionStatus {
            enabled: self.enabled.load(Ordering::Acquire),
            phase: runtime.phase,
            message: runtime.message,
            model_name: MODEL_NAME.into(),
            model_installed: self.model_path().is_file(),
            downloaded_bytes: runtime.downloaded_bytes,
            download_total_bytes: runtime.download_total_bytes,
            queued: queue.queued,
            completed: queue.completed,
            failed: queue.failed,
            total_segments: queue.total_segments,
            completed_segments: queue.completed_segments,
            current_title: runtime.current_title,
            current_segment: runtime.current_segment,
            current_total_segments: runtime.current_total_segments,
            current_segment_progress: self.segment_progress.load(Ordering::Acquire),
        })
    }

    fn model_path(&self) -> PathBuf {
        self.model_dir.join(MODEL_FILENAME)
    }

    async fn update_runtime(&self, update: impl FnOnce(&mut RuntimeStatus)) {
        let mut status = self.runtime.write().await;
        update(&mut status);
    }

    async fn run(self: Arc<Self>) {
        loop {
            if !self.enabled.load(Ordering::Acquire) {
                self.update_runtime(|status| {
                    status.phase = "disabled".into();
                    status.message =
                        "Local subtitle generation is disabled. Existing progress is saved.".into();
                    status.current_title = None;
                })
                .await;
                self.notify.notified().await;
                continue;
            }

            if self.should_pause_for_scan() {
                self.update_runtime(|status| {
                    status.phase = "waiting_for_scan".into();
                    status.message = "Paused while the media library is being scanned.".into();
                    status.current_title = None;
                })
                .await;
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                continue;
            }

            if let Err(error) = self.ensure_model().await {
                if !self.enabled.load(Ordering::Acquire) {
                    continue;
                }
                self.update_runtime(|status| {
                    status.phase = "error".into();
                    status.message = format!("Could not install the Whisper model: {error}");
                })
                .await;
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                continue;
            }

            if let Err(error) = self
                .library
                .enqueue_missing_transcriptions(
                    MODEL_NAME,
                    "en",
                    SEGMENT_DURATION_SECS,
                    self.skip_if_subtitles_exist.load(Ordering::Acquire),
                )
                .await
            {
                self.update_runtime(|status| {
                    status.phase = "error".into();
                    status.message = format!("Could not update the transcription queue: {error}");
                })
                .await;
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                continue;
            }

            if self.should_pause_for_streaming() {
                self.update_runtime(|status| {
                    status.phase = "waiting_for_streams".into();
                    status.message = "Paused while clients are streaming.".into();
                    status.current_title = None;
                })
                .await;
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                continue;
            }

            if self.should_pause_for_scan() {
                self.update_runtime(|status| {
                    status.phase = "waiting_for_scan".into();
                    status.message = "Paused while the media library is being scanned.".into();
                    status.current_title = None;
                })
                .await;
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                continue;
            }

            let job = match self.library.claim_next_transcription().await {
                Ok(Some(job)) => job,
                Ok(None) => {
                    self.update_runtime(|status| {
                        status.phase = "idle".into();
                        status.message =
                            "All eligible movies and episodes have been processed.".into();
                        status.current_title = None;
                    })
                    .await;
                    tokio::select! {
                        _ = self.notify.notified() => {},
                        _ = tokio::time::sleep(std::time::Duration::from_secs(15)) => {},
                    }
                    continue;
                }
                Err(error) => {
                    tracing::error!(%error, "could not claim transcription job");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    continue;
                }
            };

            if let Err(error) = self.process_job(&job).await {
                if error == "interrupted" {
                    let _ = self.library.requeue_transcription(&job.entry_key).await;
                } else if error.starts_with(MEDIA_ROOT_UNAVAILABLE_PREFIX) {
                    // A disconnected drive/share is temporary infrastructure
                    // state, not a bad media file. Preserve durable progress
                    // and retry instead of burning through the queue as
                    // permanently failed while the mount is absent.
                    let _ = self.library.requeue_transcription(&job.entry_key).await;
                    self.update_runtime(|status| {
                        status.phase = "waiting_for_media".into();
                        status.message = error.clone();
                        status.current_title = None;
                    })
                    .await;
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                } else {
                    tracing::warn!(entry_key = %job.entry_key, %error, "local subtitle generation failed");
                    let _ = self
                        .library
                        .fail_transcription(&job.entry_key, &error)
                        .await;
                    self.update_runtime(|status| {
                        status.phase = "error".into();
                        status.message = format!("Subtitle generation failed: {error}");
                    })
                    .await;
                }
            }
        }
    }

    async fn ensure_model(&self) -> Result<(), String> {
        let final_path = self.model_path();
        if final_path.is_file() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.model_dir).map_err(|error| error.to_string())?;
        let partial_path = final_path.with_extension("bin.part");
        let existing = std::fs::metadata(&partial_path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        self.update_runtime(|status| {
            status.phase = "downloading_model".into();
            status.message = "Downloading the compact English Whisper model. This happens once and is about 142 MB.".into();
            status.downloaded_bytes = existing;
        })
        .await;

        let client = reqwest::Client::builder()
            .user_agent("SWARM Media Server/0.1 (+https://github.com/Jerrod/swarm)")
            .build()
            .map_err(|error| error.to_string())?;
        let mut request = client.get(MODEL_URL);
        if existing > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={existing}-"));
        }
        let response = request.send().await.map_err(|error| error.to_string())?;
        if response.status() == reqwest::StatusCode::RANGE_NOT_SATISFIABLE && existing > 0 {
            verify_and_install_model(&partial_path, &final_path).await?;
            return Ok(());
        }
        if !response.status().is_success() {
            return Err(format!("model download returned {}", response.status()));
        }
        let resumed = response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        let content_length = response.content_length().unwrap_or(0);
        let total = if resumed {
            existing.saturating_add(content_length)
        } else {
            content_length
        };
        let mut downloaded = if resumed { existing } else { 0 };
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(resumed)
            .truncate(!resumed)
            .open(&partial_path)
            .await
            .map_err(|error| error.to_string())?;
        self.update_runtime(|status| {
            status.downloaded_bytes = downloaded;
            status.download_total_bytes = total;
        })
        .await;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            if !self.enabled.load(Ordering::Acquire) {
                file.flush().await.map_err(|error| error.to_string())?;
                return Err("download paused".into());
            }
            let chunk = chunk.map_err(|error| error.to_string())?;
            file.write_all(&chunk)
                .await
                .map_err(|error| error.to_string())?;
            downloaded = downloaded.saturating_add(chunk.len() as u64);
            self.update_runtime(|status| status.downloaded_bytes = downloaded)
                .await;
        }
        file.flush().await.map_err(|error| error.to_string())?;
        drop(file);

        self.update_runtime(|status| {
            status.phase = "verifying_model".into();
            status.message = "Verifying the downloaded Whisper model…".into();
        })
        .await;
        verify_and_install_model(&partial_path, &final_path).await?;
        self.update_runtime(|status| {
            status.phase = "preparing".into();
            status.message = "Whisper model installed. Preparing the transcription queue…".into();
        })
        .await;
        Ok(())
    }

    async fn process_job(&self, job: &TranscriptionJob) -> Result<(), String> {
        let entry = self
            .library
            .get(&job.entry_key)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "media entry no longer exists".to_string())?;
        if entry.fingerprint != job.fingerprint {
            return Err("media changed while it was queued".into());
        }
        let title = entry.scraped_title.clone().unwrap_or(entry.title.clone());
        let (root_path, _) = self.roots.split(&entry.relative_path);
        let media_path = self.roots.resolve(&entry.relative_path);
        for segment_index in job.completed_segments..job.total_segments {
            if !self.enabled.load(Ordering::Acquire)
                || self.should_pause_for_streaming()
                || self.should_pause_for_scan()
            {
                return Err("interrupted".into());
            }
            ensure_media_root_available(&root_path)?;
            self.update_runtime(|status| {
                status.phase = "transcribing".into();
                status.message = format!("Generating subtitles for {title}");
                status.current_title = Some(title.clone());
                status.current_segment = segment_index + 1;
                status.current_total_segments = job.total_segments;
            })
            .await;
            self.segment_progress.store(0, Ordering::Release);
            let start_secs = u64::from(segment_index) * SEGMENT_DURATION_SECS;
            let audio = match extract_audio(
                &self.ffmpeg_path,
                &media_path,
                start_secs,
                SEGMENT_DURATION_SECS,
                Arc::clone(&self.scan_active),
            )
            .await
            {
                Ok(audio) => audio,
                Err(error) => {
                    // The share can disappear between the preflight check
                    // and ffmpeg opening the file. Re-check the owning root
                    // so an outage is retried, while a genuinely bad or
                    // deleted individual file still fails normally.
                    ensure_media_root_available(&root_path)?;
                    return Err(error);
                }
            };
            // A scan may have started while ffmpeg was reading this segment.
            // Drop the buffer before loading Whisper's model/state.
            if self.should_pause_for_scan() {
                return Err("interrupted".into());
            }
            let model_path = self.model_path();
            let abort_requested = Arc::new(AtomicBool::new(false));
            let monitor_abort = Arc::clone(&abort_requested);
            let monitor_enabled = Arc::clone(&self.enabled);
            let monitor_pause_while_streaming = Arc::clone(&self.pause_while_streaming);
            let monitor_transcodes = Arc::clone(&self.transcodes);
            let monitor_scan_active = Arc::clone(&self.scan_active);
            let abort_monitor = tokio::spawn(async move {
                loop {
                    if !monitor_enabled.load(Ordering::Acquire)
                        || monitor_scan_active.load(Ordering::Acquire)
                        || (monitor_pause_while_streaming.load(Ordering::Acquire)
                            && monitor_transcodes.active_sessions() > 0)
                    {
                        monitor_abort.store(true, Ordering::Release);
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                }
            });
            let progress = Arc::clone(&self.segment_progress);
            let language = job.language.clone();
            let transcription_result = tokio::task::spawn_blocking(move || {
                transcribe_segment(
                    &model_path,
                    audio,
                    start_secs * 1_000,
                    &language,
                    abort_requested,
                    progress,
                )
            })
            .await
            .map_err(|error| error.to_string());
            abort_monitor.abort();
            let _ = abort_monitor.await;
            let cues = transcription_result??;
            let json = serde_json::to_string(&cues).map_err(|error| error.to_string())?;
            self.library
                .store_transcription_segment(&job.entry_key, segment_index, &json)
                .await
                .map_err(|error| error.to_string())?;
        }

        self.update_runtime(|status| {
            status.phase = "finalizing".into();
            status.message = format!("Finalizing subtitles for {title}");
        })
        .await;
        let segments = self
            .library
            .transcription_segments(&job.entry_key)
            .await
            .map_err(|error| error.to_string())?;
        let mut cues = Vec::new();
        for (_, json) in segments {
            cues.extend(
                serde_json::from_str::<Vec<Cue>>(&json).map_err(|error| error.to_string())?,
            );
        }
        cues.sort_by_key(|cue| cue.start_ms);
        let final_path = whisper_subtitle_path(&media_path);
        let mut partial_name = final_path.file_name().unwrap_or_default().to_os_string();
        partial_name.push(".part");
        let partial_path = final_path.with_file_name(partial_name);
        tokio::fs::write(&partial_path, render_webvtt(&cues))
            .await
            .map_err(|error| error.to_string())?;
        tokio::fs::rename(&partial_path, &final_path)
            .await
            .map_err(|error| error.to_string())?;
        self.library
            .complete_transcription(&SubtitleRecord {
                id: "whisper-en".into(),
                entry_key: job.entry_key.clone(),
                language: "en".into(),
                label: "English — AI generated".into(),
                source: "whisper".into(),
                format: "vtt".into(),
                file_path: final_path.to_string_lossy().to_string(),
                fingerprint: job.fingerprint.clone(),
            })
            .await
            .map_err(|error| error.to_string())?;
        self.segment_progress.store(100, Ordering::Release);
        Ok(())
    }
}

/// Where a Whisper-generated subtitle for `media_path` lives: alongside the
/// source file, same name minus its extension, so it travels with the media
/// if it's ever moved or copied and needs no server-owned storage of its
/// own. Shared with `bin/migrate_whisper_subtitles.rs`, which moves subtitles
/// generated under the old app-data location to this one.
pub fn whisper_subtitle_path(media_path: &Path) -> PathBuf {
    let stem = media_path.file_stem().unwrap_or_default();
    let mut filename = stem.to_os_string();
    filename.push("-whisper-english-subtitles.vtt");
    media_path.with_file_name(filename)
}

fn ensure_media_root_available(root: &Path) -> Result<(), String> {
    std::fs::read_dir(root).map(|_| ()).map_err(|error| {
        format!(
            "{MEDIA_ROOT_UNAVAILABLE_PREFIX} {}. Reconnect the drive or network share ({error})",
            root.display()
        )
    })
}

async fn extract_audio(
    ffmpeg: &Path,
    media: &Path,
    start_secs: u64,
    duration_secs: u64,
    scan_active: Arc<AtomicBool>,
) -> Result<Vec<f32>, String> {
    use std::process::Stdio;

    let start = start_secs.to_string();
    let duration = duration_secs.to_string();
    let mut child = tokio::process::Command::new(ffmpeg)
        .args(["-v", "error", "-ss", &start, "-t", &duration, "-i"])
        .arg(media)
        .args(["-vn", "-ac", "1", "-ar", "16000", "-f", "s16le", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("could not start ffmpeg: {error}"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "could not capture ffmpeg audio".to_string())?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| "could not capture ffmpeg errors".to_string())?;
    let stderr_reader = tokio::spawn(async move {
        let mut bytes = Vec::new();
        let result = stderr.read_to_end(&mut bytes).await;
        (result, bytes)
    });

    let expected_samples = duration_secs.saturating_mul(16_000).min(usize::MAX as u64) as usize;
    let mut audio = Vec::with_capacity(expected_samples);
    let mut buffer = [0u8; 64 * 1024];
    let mut carry = None;
    loop {
        let read = tokio::select! {
            read = stdout.read(&mut buffer) => read,
            _ = wait_for_scan(&scan_active) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                let _ = stderr_reader.await;
                return Err("interrupted".into());
            }
        }
        .map_err(|error| format!("could not read ffmpeg audio: {error}"))?;
        if read == 0 {
            break;
        }
        let mut index = 0;
        if let Some(low) = carry.take() {
            let sample = i16::from_le_bytes([low, buffer[0]]);
            audio.push(f32::from(sample) / 32_768.0);
            index = 1;
        }
        while index + 1 < read {
            let sample = i16::from_le_bytes([buffer[index], buffer[index + 1]]);
            audio.push(f32::from(sample) / 32_768.0);
            index += 2;
        }
        if index < read {
            carry = Some(buffer[index]);
        }
    }
    let status = child
        .wait()
        .await
        .map_err(|error| format!("could not wait for ffmpeg: {error}"))?;
    let (stderr_result, stderr_bytes) = stderr_reader
        .await
        .map_err(|error| format!("could not join ffmpeg error reader: {error}"))?;
    stderr_result.map_err(|error| format!("could not read ffmpeg errors: {error}"))?;
    if !status.success() {
        return Err(format!(
            "ffmpeg audio extraction failed: {}",
            String::from_utf8_lossy(&stderr_bytes).trim()
        ));
    }
    Ok(audio)
}

async fn wait_for_scan(scan_active: &AtomicBool) {
    while !scan_active.load(Ordering::Acquire) {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

fn transcribe_segment(
    model_path: &Path,
    audio: Vec<f32>,
    offset_ms: u64,
    language: &str,
    abort_requested: Arc<AtomicBool>,
    progress: Arc<AtomicU32>,
) -> Result<Vec<Cue>, String> {
    let context = WhisperContext::new_with_params(
        model_path.to_string_lossy().as_ref(),
        WhisperContextParameters::default(),
    )
    .map_err(|error| error.to_string())?;
    let mut state = context.create_state().map_err(|error| error.to_string())?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    let threads = std::thread::available_parallelism()
        .map(|count| count.get().div_ceil(2).clamp(1, 8) as i32)
        .unwrap_or(4);
    params.set_n_threads(threads);
    params.set_language(Some(language));
    params.set_translate(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    let callback_progress = Arc::clone(&progress);
    params.set_progress_callback_safe(move |percent: i32| {
        callback_progress.store(percent.clamp(0, 100) as u32, Ordering::Release);
    });
    // whisper-rs 0.16's safe helper stores a boxed trait-object pointer but
    // installs a trampoline for the closure's concrete type. That mismatch is
    // undefined behavior and caused a reproducible SIGBUS on macOS. Keep the
    // native callback limited to one stable atomic pointer instead.
    unsafe {
        params.set_abort_callback(Some(transcription_abort_callback));
        params.set_abort_callback_user_data(
            Arc::as_ptr(&abort_requested).cast_mut().cast::<c_void>(),
        );
    }
    if let Err(error) = state.full(params, &audio) {
        if abort_requested.load(Ordering::Acquire) {
            return Err("interrupted".into());
        }
        return Err(error.to_string());
    }
    progress.store(100, Ordering::Release);
    let mut cues = Vec::new();
    for segment in state.as_iter() {
        let text = segment
            .to_str_lossy()
            .map_err(|error| error.to_string())?
            .trim()
            .to_string();
        if text.is_empty() {
            continue;
        }
        cues.push(Cue {
            start_ms: offset_ms + (segment.start_timestamp().max(0) as u64 * 10),
            end_ms: offset_ms + (segment.end_timestamp().max(0) as u64 * 10),
            text,
        });
    }
    Ok(cues)
}

/// # Safety
///
/// `user_data` must point to an `AtomicBool` that stays alive throughout the
/// synchronous `WhisperState::full` call. `transcribe_segment` retains the
/// owning `Arc` until that call returns.
unsafe extern "C" fn transcription_abort_callback(user_data: *mut c_void) -> bool {
    if user_data.is_null() {
        return false;
    }
    // SAFETY: upheld by the synchronous call and retained Arc above.
    unsafe { &*user_data.cast::<AtomicBool>() }.load(Ordering::Acquire)
}

fn render_webvtt(cues: &[Cue]) -> String {
    let mut output = String::from("WEBVTT\n\n");
    for (index, cue) in cues.iter().enumerate() {
        output.push_str(&(index + 1).to_string());
        output.push('\n');
        output.push_str(&format!(
            "{} --> {}\n{}\n\n",
            vtt_timestamp(cue.start_ms),
            vtt_timestamp(cue.end_ms),
            cue.text
        ));
    }
    output
}

fn vtt_timestamp(milliseconds: u64) -> String {
    let hours = milliseconds / 3_600_000;
    let minutes = (milliseconds / 60_000) % 60;
    let seconds = (milliseconds / 1_000) % 60;
    let millis = milliseconds % 1_000;
    format!("{hours:02}:{minutes:02}:{seconds:02}.{millis:03}")
}

async fn verify_sha1(path: &Path, expected: &str) -> Result<(), String> {
    use sha1::{Digest as Sha1Digest, Sha1};
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| error.to_string())?;
    let mut digest = Sha1::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    let actual = hex::encode(digest.finalize());
    if actual != expected {
        return Err(format!(
            "model checksum mismatch (expected {expected}, got {actual})"
        ));
    }
    Ok(())
}

async fn verify_and_install_model(partial_path: &Path, final_path: &Path) -> Result<(), String> {
    if let Err(error) = verify_sha1(partial_path, MODEL_SHA1).await {
        // A bad completed partial can otherwise get a 416 on every resume
        // attempt forever. Delete only this server-owned download and retry
        // cleanly on the worker's next pass.
        let _ = tokio::fs::remove_file(partial_path).await;
        return Err(error);
    }
    tokio::fs::rename(partial_path, final_path)
        .await
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whisper_subtitle_path_sits_alongside_the_source_media() {
        let media = Path::new("/library/Movies/Inception (2010)/Inception (2010).mkv");
        let subtitle = whisper_subtitle_path(media);
        assert_eq!(
            subtitle,
            Path::new(
                "/library/Movies/Inception (2010)/Inception (2010)-whisper-english-subtitles.vtt"
            )
        );
    }

    #[test]
    fn webvtt_uses_absolute_segment_timestamps() {
        let text = render_webvtt(&[Cue {
            start_ms: 3_723_004,
            end_ms: 3_725_006,
            text: "Hello".into(),
        }]);
        assert!(text.contains("01:02:03.004 --> 01:02:05.006"));
        assert!(text.contains("Hello"));
    }

    #[test]
    fn native_abort_callback_reads_a_stable_atomic_flag() {
        let abort = Arc::new(AtomicBool::new(false));
        let user_data = Arc::as_ptr(&abort).cast_mut().cast::<c_void>();
        // SAFETY: `abort` owns this pointer for both synchronous calls.
        assert!(!unsafe { transcription_abort_callback(user_data) });
        abort.store(true, Ordering::Release);
        // SAFETY: same retained Arc and pointer as above.
        assert!(unsafe { transcription_abort_callback(user_data) });
        // SAFETY: null is explicitly handled as "continue".
        assert!(!unsafe { transcription_abort_callback(std::ptr::null_mut()) });
    }

    #[test]
    fn inaccessible_media_root_is_reported_as_retryable_storage_state() {
        let missing = std::env::temp_dir().join(format!(
            "swarm-missing-transcription-root-{}",
            rand::random::<u64>()
        ));
        let error = ensure_media_root_available(&missing).unwrap_err();
        assert!(error.starts_with(MEDIA_ROOT_UNAVAILABLE_PREFIX));
        assert!(error.contains("Reconnect the drive or network share"));
    }
}
