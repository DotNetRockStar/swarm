//! Periodic real upload-bandwidth measurement, so the streaming budget
//! (`TranscodeManager::set_max_upload_bps`) tracks this machine's actual
//! uplink instead of a static per-install guess that's wrong the moment
//! it's run on a connection faster or slower than whatever default was
//! picked — every SWARM install has a different real uplink, so this has
//! to be measured on each one, not hardcoded anywhere.

use crate::state_db::StateDb;
use std::sync::Arc;
use std::time::Duration;
use swarm_media::transcode::TranscodeManager;

/// Cloudflare's public speed-test upload endpoint — provisioned by
/// Cloudflare specifically for exactly this kind of one-shot throughput
/// probe (no auth required, meant for repeated real-world use), the same
/// endpoint open-source speed-test tools use for their own upload-direction
/// tests.
const PROBE_URL: &str = "https://speed.cloudflare.com/__up";
/// Large enough to get past TCP slow-start and settle into a real
/// steady-state rate; small enough not to waste meaningful real data on
/// every interval.
const PROBE_PAYLOAD_BYTES: usize = 16_000_000;

pub const DEFAULT_PROBE_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// `SWARM_BANDWIDTH_PROBE_INTERVAL_SECS`, defaulting to [DEFAULT_PROBE_INTERVAL] (60 minutes).
pub fn interval_from_env() -> Duration {
    std::env::var("SWARM_BANDWIDTH_PROBE_INTERVAL_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_PROBE_INTERVAL)
}

#[derive(Debug, thiserror::Error)]
pub enum BandwidthProbeError {
    #[error("probe request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("probe upload took no measurable time")]
    NoElapsedTime,
}

/// Uploads a real payload to a real internet endpoint and returns the
/// measured throughput in bits/sec. Network-bound and deliberately slow
/// (it has to actually push real bytes to get a real number) — always call
/// from a background task, never inline with a request a client is
/// waiting on.
pub async fn measure_upload_bps(client: &reqwest::Client) -> Result<u64, BandwidthProbeError> {
    let payload = vec![0u8; PROBE_PAYLOAD_BYTES];
    let started = std::time::Instant::now();
    client
        .post(PROBE_URL)
        .body(payload)
        .send()
        .await?
        .error_for_status()?;
    let elapsed = started.elapsed();
    if elapsed.is_zero() {
        return Err(BandwidthProbeError::NoElapsedTime);
    }
    Ok((PROBE_PAYLOAD_BYTES as f64 * 8.0 / elapsed.as_secs_f64()) as u64)
}

/// Runs forever: measures immediately (so a fresh process doesn't sit on
/// the static default for a full `interval` before its first real reading),
/// applies the result to `transcodes` and persists it, then repeats every
/// `interval`. A failed measurement (no internet, endpoint unreachable) is
/// logged and skipped — the previous baseline (or the static default, on a
/// fresh install with no measurement history yet) stays in effect rather
/// than zeroing out the streaming budget over what might just be a
/// transient outage.
pub async fn run_periodic_probe(
    state_db: Arc<StateDb>,
    transcodes: Arc<TranscodeManager>,
    interval: Duration,
) {
    let client = reqwest::Client::new();
    loop {
        match measure_upload_bps(&client).await {
            Ok(bps) => {
                tracing::info!(upload_bps = bps, "measured real upload bandwidth");
                transcodes.set_max_upload_bps(bps);
                if let Err(err) = state_db.record_bandwidth_measurement(bps).await {
                    tracing::warn!(%err, "could not persist bandwidth measurement");
                }
            }
            Err(err) => {
                tracing::warn!(%err, "upload bandwidth probe failed; keeping the previous baseline");
            }
        }
        tokio::time::sleep(interval).await;
    }
}
