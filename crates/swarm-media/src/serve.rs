//! The peer-facing media service: maps `PeerRequest`s onto the library and
//! the filesystem, and the QUIC accept loop that runs it.
//!
//! Path safety follows the Drone discipline: entry keys are validated as
//! lowercase hex *before* any lookup, and the file path served always comes
//! from the library row (derived from the scanned relative path under the
//! media root) — never from request input.

use crate::range::{content_type, resolve, ResolvedRange};
use crate::roots::RootResolver;
use crate::store::{ArtworkKind, Library};
use crate::transcode::{
    hls_content_type, SessionRateLimiter, TranscodeConfig, TranscodeError, TranscodeManager,
};
use std::path::PathBuf;
use std::sync::Arc;
use swarm_core::entry_key::is_valid_entry_key;
use swarm_core::peer::{CatalogManifest, CatalogThumbprint, PeerRequest, PeerResponseHeader};
use swarm_p2p::endpoint::{read_request, write_response_header, P2pError};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

pub struct MediaService {
    library: Arc<Library>,
    roots: RootResolver,
    transcodes: Arc<TranscodeManager>,
}

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
        Self::with_roots(library, RootResolver::single(media_root), config)
    }

    /// Multi-root variant of [`Self::with_transcoding`] — see `crate::roots`.
    pub fn with_roots(library: Arc<Library>, roots: RootResolver, config: TranscodeConfig) -> Self {
        Self {
            library,
            roots,
            transcodes: TranscodeManager::new(config),
        }
    }

    pub fn transcode_manager(&self) -> &Arc<TranscodeManager> {
        &self.transcodes
    }

    pub async fn resolve(&self, request: &PeerRequest) -> Resolved {
        let request_path = request
            .path
            .split_once('?')
            .map(|(path, _)| path)
            .unwrap_or(request.path.as_str());
        match request_path {
            "/catalog/thumbprint" => self.thumbprint().await,
            "/catalog/manifest" => self.manifest().await,
            path => {
                if let Some(entry_key) = path.strip_prefix("/media/") {
                    self.media(entry_key, request).await
                } else if let Some(entry_key) = path.strip_prefix("/play/") {
                    self.play(entry_key, request).await
                } else if let Some(rest) = path.strip_prefix("/stream/") {
                    self.session_media(rest, request).await
                } else if let Some(rest) = path.strip_prefix("/hls/") {
                    self.hls(rest, request).await
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
        match (
            self.library.thumbprint().await,
            self.library.entry_count().await,
        ) {
            (Ok(thumbprint), Ok(entry_count)) => json_response(
                200,
                &CatalogThumbprint {
                    thumbprint,
                    entry_count,
                },
            ),
            _ => status(500),
        }
    }

    async fn manifest(&self) -> Resolved {
        let (Ok(thumbprint), Ok(entries)) =
            (self.library.thumbprint().await, self.library.list().await)
        else {
            return status(500);
        };
        let manifest = CatalogManifest {
            thumbprint,
            entries: entries.iter().map(|e| e.to_catalog_entry()).collect(),
            removed: Vec::new(),
        };
        json_response(200, &manifest)
    }

    async fn media(&self, entry_key: &str, request: &PeerRequest) -> Resolved {
        if !is_valid_entry_key(entry_key) {
            return status(404);
        }
        let Ok(Some(entry)) = self.library.get(entry_key).await else {
            return status(404);
        };
        self.media_entry(
            entry,
            request,
            None,
            vec![self.transcodes.global_rate_limiter()],
        )
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

    async fn play(&self, entry_key: &str, request: &PeerRequest) -> Resolved {
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
        match self.transcodes.plan(&entry, &media_path, preferences).await {
            Ok(plan) => json_response(200, &plan),
            Err(error) => {
                tracing::warn!(entry_key, %error, "playback negotiation failed");
                transcode_error(error)
            }
        }
    }

    async fn session_media(&self, rest: &str, request: &PeerRequest) -> Resolved {
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
            vec![self.transcodes.global_rate_limiter(), rate_limiter],
        )
        .await
    }

    async fn hls(&self, rest: &str, request: &PeerRequest) -> Resolved {
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
                    rate_limiters: vec![self.transcodes.global_rate_limiter(), file.rate_limiter],
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
                        rate_limiters: vec![
                            self.transcodes.global_rate_limiter(),
                            file.rate_limiter,
                        ],
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

    /// `GET /art/{entry_key}/{poster|backdrop|cover|artist}` — the artwork a
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
        let etag = format!("v{version}");
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
        let path = self.roots.resolve(&relative_path);
        let Ok(metadata) = std::fs::metadata(&path) else {
            return status(404); // artwork file missing from disk since the scrape
        };
        let total = metadata.len();
        match resolve(request.range, total) {
            ResolvedRange::Full { len } => Resolved {
                header: PeerResponseHeader {
                    status: 200,
                    len,
                    content_type: Some(image_content_type(&relative_path).into()),
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
                        content_type: Some(image_content_type(&relative_path).into()),
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

/// Serve one accepted bidi stream: read the request, resolve it, stream the
/// body out in 64 KiB chunks.
pub async fn handle_stream(
    service: &MediaService,
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
) -> Result<(), P2pError> {
    let request = read_request(&mut recv).await?;
    let resolved = service.resolve(&request).await;
    let session_id = resolved.session_id.clone();
    let result = async {
        write_response_header(&mut send, &resolved.header).await?;
        match resolved.body {
            Body::Bytes(bytes) => {
                send.write_all(&bytes).await?;
            }
            Body::File {
                path,
                offset,
                len,
                rate_limiters,
            } => {
                let mut file = tokio::fs::File::open(&path).await?;
                file.seek(std::io::SeekFrom::Start(offset)).await?;
                let mut remaining = len;
                let mut buffer = vec![0u8; 64 * 1024];
                while remaining > 0 {
                    let want = buffer.len().min(remaining as usize);
                    let got = file.read(&mut buffer[..want]).await?;
                    if got == 0 {
                        return Err(P2pError::Protocol("file truncated while serving".into()));
                    }
                    for limiter in &rate_limiters {
                        limiter.wait_for(got).await;
                    }
                    send.write_all(&buffer[..got]).await?;
                    remaining -= got as u64;
                }
            }
        }
        send.finish().ok();
        Ok(())
    }
    .await;
    if let Some(session_id) = session_id {
        service.transcodes.finish_use(&session_id);
    }
    result
}

/// Serve every request stream an already-established connection sends,
/// spawning a task per stream, until the peer closes it. Split out from
/// [`accept_loop`] so a connection that arrived some other way — a punched
/// connection from `apps/server`'s `punch_connect`, say, rather than
/// `endpoint.accept()` — gets exactly the same per-stream serving behavior.
pub async fn serve_connection(connection: quinn::Connection, service: Arc<MediaService>) {
    tracing::info!(remote = %connection.remote_address(), "peer connected");
    // Loop ends when accept_bi errors, i.e. the connection closed.
    while let Ok((send, recv)) = connection.accept_bi().await {
        let service = Arc::clone(&service);
        tokio::spawn(async move {
            if let Err(err) = handle_stream(&service, send, recv).await {
                tracing::debug!(error = %err, "stream failed");
            }
        });
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
