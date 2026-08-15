//! The peer-facing media service: maps `PeerRequest`s onto the library and
//! the filesystem, and the QUIC accept loop that runs it.
//!
//! Path safety follows the Drone discipline: entry keys are validated as
//! lowercase hex *before* any lookup, and the file path served always comes
//! from the library row (derived from the scanned relative path under the
//! media root) — never from request input.

use crate::range::{content_type, resolve, ResolvedRange};
use crate::store::Library;
use std::path::PathBuf;
use std::sync::Arc;
use swarm_core::entry_key::is_valid_entry_key;
use swarm_core::peer::{
    CatalogManifest, CatalogThumbprint, PeerRequest, PeerResponseHeader,
};
use swarm_p2p::endpoint::{read_request, write_response_header, P2pError};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

pub struct MediaService {
    library: Arc<Library>,
    media_root: PathBuf,
}

/// A resolved response: header plus a body source the transport streams out.
pub enum Body {
    Bytes(Vec<u8>),
    File { path: PathBuf, offset: u64, len: u64 },
}

pub struct Resolved {
    pub header: PeerResponseHeader,
    pub body: Body,
}

fn status(status: u16) -> Resolved {
    Resolved {
        header: PeerResponseHeader { status, len: 0, content_type: None, content_range: None, etag: None },
        body: Body::Bytes(Vec::new()),
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
    }
}

impl MediaService {
    pub fn new(library: Arc<Library>, media_root: PathBuf) -> Self {
        Self { library, media_root }
    }

    pub async fn resolve(&self, request: &PeerRequest) -> Resolved {
        match request.path.as_str() {
            "/catalog/thumbprint" => self.thumbprint().await,
            "/catalog/manifest" => self.manifest().await,
            path => {
                if let Some(entry_key) = path.strip_prefix("/media/") {
                    self.media(entry_key, request).await
                } else {
                    status(404)
                }
            }
        }
    }

    async fn thumbprint(&self) -> Resolved {
        match (self.library.thumbprint().await, self.library.entry_count().await) {
            (Ok(thumbprint), Ok(entry_count)) => {
                json_response(200, &CatalogThumbprint { thumbprint, entry_count })
            }
            _ => status(500),
        }
    }

    async fn manifest(&self) -> Resolved {
        let (Ok(thumbprint), Ok(entries)) = (self.library.thumbprint().await, self.library.list().await) else {
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
        let path = self.media_root.join(&entry.relative_path);
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
                body: Body::File { path, offset: 0, len },
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
                    body: Body::File { path, offset: content_range.start, len },
                }
            }
            ResolvedRange::Unsatisfiable => status(416),
        }
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
    write_response_header(&mut send, &resolved.header).await?;
    match resolved.body {
        Body::Bytes(bytes) => {
            send.write_all(&bytes).await?;
        }
        Body::File { path, offset, len } => {
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
                send.write_all(&buffer[..got]).await?;
                remaining -= got as u64;
            }
        }
    }
    send.finish().ok();
    Ok(())
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
        });
    }
}
