//! Device registration and management.
//!
//! Registration redeems a single-use, swarm-scoped join code (constant-time
//! compare on the hash, per the drone's pairing-code discipline) and returns
//! the access token exactly once. Re-registering the same (user, machine_id)
//! updates the existing device row — same machine, fresh token/cert — instead
//! of minting duplicates.

use crate::authn::{require_csrf, require_device, require_session};
use crate::db::{now, rfc3339};
use crate::error::{ApiResult, AppError};
use crate::routes::swarms::{device_type_str, parse_device_type};
use crate::security::{generate_token, new_id, token_hash};
use crate::state::SharedState;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::HeaderMap;
use axum::Json;
use axum_extra::extract::cookie::CookieJar;
use serde::Serialize;
use sqlx::SqliteConnection;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use swarm_core::rest::{
    DeviceRegistration, JoinSwarmRequest, MetadataPatchRequest, RegisterDeviceRequest,
    RegisterDeviceResponse, SwarmSummary,
};
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct MyDevice {
    pub device_id: String,
    pub name: String,
    pub device_type: swarm_core::rest::DeviceType,
    pub platform: String,
    pub online: bool,
    pub last_seen_at: Option<String>,
    pub swarms: Vec<SwarmSummary>,
}

#[derive(Serialize, ToSchema)]
pub struct MyDevicesResponse {
    pub devices: Vec<MyDevice>,
}

pub(crate) fn validate_registration(device: &DeviceRegistration) -> Result<(), AppError> {
    let fp = &device.cert_fingerprint;
    if fp.len() != 64
        || !fp
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(AppError::bad_request(
            "invalid_fingerprint",
            "cert_fingerprint must be 64 lowercase hex chars",
        ));
    }
    if device.name.trim().is_empty() || device.name.len() > 64 {
        return Err(AppError::bad_request(
            "invalid_name",
            "device name must be 1-64 characters",
        ));
    }
    if device.machine_id.trim().is_empty() || device.machine_id.len() > 128 {
        return Err(AppError::bad_request(
            "invalid_machine_id",
            "machine_id must be 1-128 characters",
        ));
    }
    if device.metadata.len() > 64 {
        return Err(AppError::bad_request(
            "too_much_metadata",
            "at most 64 metadata keys",
        ));
    }
    for (key, value) in &device.metadata {
        if key.is_empty() || key.len() > 64 || value.len() > 1024 {
            return Err(AppError::bad_request(
                "invalid_metadata",
                "metadata keys 1-64 chars, values <= 1024 chars",
            ));
        }
    }
    Ok(())
}

/// Look up an unexpired, unredeemed join code and mark it redeemed. The
/// SHA-256 lookup is effectively constant-time over the candidate set; the
/// UPDATE's `redeemed_by_device IS NULL` guard closes the double-redeem race.
async fn redeem_code(
    connection: &mut SqliteConnection,
    code: &str,
    device_id: &str,
) -> ApiResult<SwarmSummary> {
    let hash = token_hash(code.trim());
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT jc.id, s.id, s.name FROM join_codes jc JOIN swarms s ON s.id = jc.swarm_id \
         WHERE jc.code_hash = ? AND jc.expires_at >= ? AND jc.redeemed_by_device IS NULL",
    )
    .bind(&hash)
    .bind(now())
    .fetch_optional(&mut *connection)
    .await?;
    let Some((code_id, swarm_id, swarm_name)) = row else {
        return Err(AppError::unauthorized(
            "join code is invalid, expired, or already used",
        ));
    };
    let claimed = sqlx::query(
        "UPDATE join_codes SET redeemed_by_device = ? WHERE id = ? AND redeemed_by_device IS NULL",
    )
    .bind(device_id)
    .bind(&code_id)
    .execute(&mut *connection)
    .await?;
    if claimed.rows_affected() == 0 {
        return Err(AppError::unauthorized(
            "join code is invalid, expired, or already used",
        ));
    }
    Ok(SwarmSummary {
        id: swarm_id,
        name: swarm_name,
    })
}

pub(crate) async fn add_membership(
    connection: &mut SqliteConnection,
    swarm_id: &str,
    device_id: &str,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO swarm_devices (swarm_id, device_id, joined_at) VALUES (?, ?, ?)",
    )
    .bind(swarm_id)
    .bind(device_id)
    .bind(now())
    .execute(connection)
    .await?;
    Ok(())
}

pub(crate) async fn replace_metadata(
    connection: &mut SqliteConnection,
    device_id: &str,
    metadata: &BTreeMap<String, String>,
) -> ApiResult<()> {
    sqlx::query("DELETE FROM device_metadata WHERE device_id = ?")
        .bind(device_id)
        .execute(&mut *connection)
        .await?;
    for (key, value) in metadata {
        sqlx::query("INSERT INTO device_metadata (device_id, key, value) VALUES (?, ?, ?)")
            .bind(device_id)
            .bind(key)
            .bind(value)
            .execute(&mut *connection)
            .await?;
    }
    Ok(())
}

#[utoipa::path(post, path = "/api/v1/devices/register", request_body = RegisterDeviceRequest,
    responses((status = 201, body = RegisterDeviceResponse), (status = 401, body = swarm_core::rest::ApiError)),
    tag = "devices")]
pub async fn register(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(req): Json<RegisterDeviceRequest>,
) -> ApiResult<(axum::http::StatusCode, Json<RegisterDeviceResponse>)> {
    if state.blocker.is_blocked(addr.ip()) {
        return Err(AppError::too_many_requests());
    }
    validate_registration(&req.device)?;

    // Device upsert, code redemption, membership, and metadata replacement
    // form one relational operation. If any step fails, no orphaned device or
    // consumed join code is left behind.
    let mut tx = state.db.begin().await?;

    // The code's swarm owner becomes the device's owning user.
    let hash = token_hash(req.code.trim());
    let owner: Option<(String,)> = sqlx::query_as(
        "SELECT s.owner_user_id FROM join_codes jc JOIN swarms s ON s.id = jc.swarm_id \
         WHERE jc.code_hash = ? AND jc.expires_at >= ? AND jc.redeemed_by_device IS NULL",
    )
    .bind(&hash)
    .bind(now())
    .fetch_optional(&mut *tx)
    .await?;
    let Some((user_id,)) = owner else {
        state.blocker.record_failure(addr.ip());
        return Err(AppError::unauthorized(
            "join code is invalid, expired, or already used",
        ));
    };

    let access_token = generate_token();
    let ts = now();
    // Upsert on (user, machine_id): same machine re-registering gets its row
    // updated (new cert fingerprint, new token, un-revoked) — the recovered
    // Overmind claim/re-register behavior.
    let device_id: String = {
        let existing: Option<(String,)> =
            sqlx::query_as("SELECT id FROM devices WHERE user_id = ? AND machine_id = ?")
                .bind(&user_id)
                .bind(&req.device.machine_id)
                .fetch_optional(&mut *tx)
                .await?;
        match existing {
            Some((id,)) => {
                sqlx::query(
                    "UPDATE devices SET name = ?, device_type = ?, cert_fingerprint = ?, platform = ?, \
                     app_version = ?, access_token_hash = ?, revoked_at = NULL, last_seen_at = ? WHERE id = ?",
                )
                .bind(req.device.name.trim())
                .bind(device_type_str(req.device.device_type))
                .bind(&req.device.cert_fingerprint)
                .bind(&req.device.platform)
                .bind(&req.device.app_version)
                .bind(token_hash(&access_token))
                .bind(ts)
                .bind(&id)
                .execute(&mut *tx)
                .await?;
                id
            }
            None => {
                let id = new_id();
                sqlx::query(
                    "INSERT INTO devices (id, user_id, name, device_type, machine_id, cert_fingerprint, \
                     platform, app_version, access_token_hash, last_seen_at, created_at) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(&id)
                .bind(&user_id)
                .bind(req.device.name.trim())
                .bind(device_type_str(req.device.device_type))
                .bind(&req.device.machine_id)
                .bind(&req.device.cert_fingerprint)
                .bind(&req.device.platform)
                .bind(&req.device.app_version)
                .bind(token_hash(&access_token))
                .bind(ts)
                .bind(ts)
                .execute(&mut *tx)
                .await?;
                id
            }
        }
    };
    let swarm = redeem_code(&mut tx, &req.code, &device_id).await?;
    add_membership(&mut tx, &swarm.id, &device_id).await?;
    replace_metadata(&mut tx, &device_id, &req.device.metadata).await?;
    tx.commit().await?;
    state.blocker.record_success(addr.ip());
    tracing::info!(device_id, swarm = %swarm.name, "device registered");
    Ok((
        axum::http::StatusCode::CREATED,
        Json(RegisterDeviceResponse {
            access_token,
            device_id,
            swarm,
        }),
    ))
}

#[utoipa::path(post, path = "/api/v1/swarms/join", request_body = JoinSwarmRequest,
    responses((status = 200, body = SwarmSummary)), security(("bearerAuth" = [])), tag = "devices")]
pub async fn join_swarm(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<JoinSwarmRequest>,
) -> ApiResult<Json<SwarmSummary>> {
    if state.blocker.is_blocked(addr.ip()) {
        return Err(AppError::too_many_requests());
    }
    let device = require_device(&state, &headers).await?;
    let mut tx = state.db.begin().await?;
    let swarm = match redeem_code(&mut tx, &req.code, &device.device_id).await {
        Ok(swarm) => swarm,
        Err(err) => {
            state.blocker.record_failure(addr.ip());
            return Err(err);
        }
    };
    add_membership(&mut tx, &swarm.id, &device.device_id).await?;
    tx.commit().await?;
    Ok(Json(swarm))
}

#[utoipa::path(get, path = "/api/v1/me/devices", responses((status = 200, body = MyDevicesResponse)), tag = "devices")]
pub async fn my_devices(
    State(state): State<SharedState>,
    jar: CookieJar,
) -> ApiResult<Json<MyDevicesResponse>> {
    let user = require_session(&state, &jar).await?;
    let rows: Vec<(String, String, String, String, Option<i64>)> = sqlx::query_as(
        "SELECT id, name, device_type, platform, last_seen_at FROM devices \
         WHERE user_id = ? AND revoked_at IS NULL ORDER BY created_at",
    )
    .bind(&user.user_id)
    .fetch_all(&state.db)
    .await?;
    let mut devices = Vec::with_capacity(rows.len());
    for (device_id, name, device_type, platform, last_seen_at) in rows {
        let swarms: Vec<(String, String)> = sqlx::query_as(
            "SELECT s.id, s.name FROM swarms s JOIN swarm_devices sd ON sd.swarm_id = s.id WHERE sd.device_id = ?",
        )
        .bind(&device_id)
        .fetch_all(&state.db)
        .await?;
        devices.push(MyDevice {
            online: state.hub.is_online(&device_id),
            name,
            device_type: parse_device_type(&device_type),
            platform,
            last_seen_at: last_seen_at.map(rfc3339),
            swarms: swarms
                .into_iter()
                .map(|(id, name)| SwarmSummary { id, name })
                .collect(),
            device_id,
        });
    }
    Ok(Json(MyDevicesResponse { devices }))
}

#[utoipa::path(delete, path = "/api/v1/devices/{device_id}",
    responses((status = 200, body = super::auth::StatusResponse)), tag = "devices")]
pub async fn revoke(
    State(state): State<SharedState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(device_id): Path<String>,
) -> ApiResult<Json<super::auth::StatusResponse>> {
    require_csrf(&jar, &headers)?;
    let user = require_session(&state, &jar).await?;
    let updated = sqlx::query("UPDATE devices SET revoked_at = ? WHERE id = ? AND user_id = ?")
        .bind(now())
        .bind(&device_id)
        .bind(&user.user_id)
        .execute(&state.db)
        .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::not_found("no such device"));
    }
    // Drop it from rosters and kick any live signaling session.
    sqlx::query("DELETE FROM swarm_devices WHERE device_id = ?")
        .bind(&device_id)
        .execute(&state.db)
        .await?;
    state.hub.send_to(
        &device_id,
        swarm_core::signal::SignalMessage::Error {
            code: "revoked".into(),
            message: "device was revoked".into(),
        },
    );
    Ok(Json(super::auth::StatusResponse {
        status: "ok".into(),
    }))
}

#[utoipa::path(patch, path = "/api/v1/devices/{device_id}/metadata", request_body = MetadataPatchRequest,
    responses((status = 200, body = super::auth::StatusResponse)), security(("bearerAuth" = [])), tag = "devices")]
pub async fn patch_metadata(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
    Json(req): Json<MetadataPatchRequest>,
) -> ApiResult<Json<super::auth::StatusResponse>> {
    let device = require_device(&state, &headers).await?;
    if device.device_id != device_id {
        return Err(AppError::forbidden(
            "a device may only update its own metadata",
        ));
    }
    for (key, value) in &req.metadata {
        if key.is_empty() || key.len() > 64 || value.len() > 1024 {
            return Err(AppError::bad_request(
                "invalid_metadata",
                "metadata keys 1-64 chars, values <= 1024 chars",
            ));
        }
        if value.is_empty() {
            sqlx::query("DELETE FROM device_metadata WHERE device_id = ? AND key = ?")
                .bind(&device_id)
                .bind(key)
                .execute(&state.db)
                .await?;
        } else {
            sqlx::query(
                "INSERT INTO device_metadata (device_id, key, value) VALUES (?, ?, ?) \
                 ON CONFLICT(device_id, key) DO UPDATE SET value = excluded.value",
            )
            .bind(&device_id)
            .bind(key)
            .bind(value)
            .execute(&state.db)
            .await?;
        }
    }
    Ok(Json(super::auth::StatusResponse {
        status: "ok".into(),
    }))
}
