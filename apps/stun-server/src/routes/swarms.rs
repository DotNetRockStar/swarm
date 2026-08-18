//! Swarm management (session-authenticated) and swarm device listings
//! (session or device Bearer).

use crate::authn::{require_csrf, require_device, require_session, session_user};
use crate::db::{now, rfc3339};
use crate::error::{ApiResult, AppError};
use crate::security::{generate_join_code, new_id, token_hash};
use crate::state::SharedState;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use axum_extra::extract::cookie::CookieJar;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use swarm_core::rest::{DeviceType, SwarmDevice, SwarmDevicesResponse, SwarmSummary};
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct SwarmListEntry {
    pub id: String,
    pub name: String,
    pub device_count: i64,
}

#[derive(Serialize, ToSchema)]
pub struct SwarmListResponse {
    pub swarms: Vec<SwarmListEntry>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateSwarmRequest {
    pub name: String,
}

#[derive(Deserialize, ToSchema, Default)]
#[serde(deny_unknown_fields, default)]
pub struct CreateCodeRequest {
    /// Optional expected device type, shown in the UI; not enforced at
    /// redemption (the device declares its real type).
    pub device_type_hint: Option<DeviceType>,
}

#[derive(Serialize, ToSchema)]
pub struct JoinCodeResponse {
    /// Shown exactly once; only a hash is stored.
    pub code: String,
    pub expires_at: String,
}

pub fn parse_device_type(raw: &str) -> DeviceType {
    match raw {
        "server" => DeviceType::Server,
        "both" => DeviceType::Both,
        _ => DeviceType::Client,
    }
}

pub fn device_type_str(dt: DeviceType) -> &'static str {
    match dt {
        DeviceType::Client => "client",
        DeviceType::Server => "server",
        DeviceType::Both => "both",
    }
}

async fn owned_swarm(state: &SharedState, user_id: &str, swarm_id: &str) -> ApiResult<SwarmSummary> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT id, name FROM swarms WHERE id = ? AND owner_user_id = ?")
            .bind(swarm_id)
            .bind(user_id)
            .fetch_optional(&state.db)
            .await?;
    row.map(|(id, name)| SwarmSummary { id, name }).ok_or_else(|| AppError::not_found("no such swarm"))
}

#[utoipa::path(get, path = "/api/v1/swarms", responses((status = 200, body = SwarmListResponse)), tag = "swarms")]
pub async fn list(State(state): State<SharedState>, jar: CookieJar) -> ApiResult<Json<SwarmListResponse>> {
    let user = require_session(&state, &jar).await?;
    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT s.id, s.name, COUNT(sd.device_id) FROM swarms s \
         LEFT JOIN swarm_devices sd ON sd.swarm_id = s.id \
         WHERE s.owner_user_id = ? GROUP BY s.id ORDER BY s.created_at",
    )
    .bind(&user.user_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(SwarmListResponse {
        swarms: rows
            .into_iter()
            .map(|(id, name, device_count)| SwarmListEntry { id, name, device_count })
            .collect(),
    }))
}

#[utoipa::path(post, path = "/api/v1/swarms", request_body = CreateSwarmRequest,
    responses((status = 201, body = SwarmSummary)), tag = "swarms")]
pub async fn create(
    State(state): State<SharedState>,
    jar: CookieJar,
    headers: HeaderMap,
    Json(req): Json<CreateSwarmRequest>,
) -> ApiResult<(axum::http::StatusCode, Json<SwarmSummary>)> {
    require_csrf(&jar, &headers)?;
    let user = require_session(&state, &jar).await?;
    let name = req.name.trim().to_string();
    if name.is_empty() || name.len() > 64 {
        return Err(AppError::bad_request("invalid_name", "swarm name must be 1-64 characters"));
    }
    let id = new_id();
    sqlx::query("INSERT INTO swarms (id, owner_user_id, name, created_at) VALUES (?, ?, ?, ?)")
        .bind(&id)
        .bind(&user.user_id)
        .bind(&name)
        .bind(now())
        .execute(&state.db)
        .await?;
    Ok((axum::http::StatusCode::CREATED, Json(SwarmSummary { id, name })))
}

#[utoipa::path(delete, path = "/api/v1/swarms/{swarm_id}",
    responses((status = 200, body = super::auth::StatusResponse)), tag = "swarms")]
pub async fn delete(
    State(state): State<SharedState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(swarm_id): Path<String>,
) -> ApiResult<Json<super::auth::StatusResponse>> {
    require_csrf(&jar, &headers)?;
    let user = require_session(&state, &jar).await?;
    owned_swarm(&state, &user.user_id, &swarm_id).await?;
    sqlx::query("DELETE FROM swarms WHERE id = ?").bind(&swarm_id).execute(&state.db).await?;
    Ok(Json(super::auth::StatusResponse { status: "ok".into() }))
}

#[utoipa::path(post, path = "/api/v1/swarms/{swarm_id}/codes", request_body = CreateCodeRequest,
    responses((status = 201, body = JoinCodeResponse)), tag = "swarms")]
pub async fn create_code(
    State(state): State<SharedState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(swarm_id): Path<String>,
    Json(req): Json<CreateCodeRequest>,
) -> ApiResult<(axum::http::StatusCode, Json<JoinCodeResponse>)> {
    require_csrf(&jar, &headers)?;
    let user = require_session(&state, &jar).await?;
    owned_swarm(&state, &user.user_id, &swarm_id).await?;
    // Sweep expired codes, then mint (the drone rotates codes aggressively;
    // here every code is independent and single-use).
    sqlx::query("DELETE FROM join_codes WHERE expires_at < ?").bind(now()).execute(&state.db).await?;
    let expires_at = now() + state.config.join_code_ttl_secs;
    let device_type_hint = req.device_type_hint.map(device_type_str);
    let mut minted_code = None;
    // The database uniquely indexes unredeemed code hashes. Retry the tiny
    // chance of an 8-digit collision instead of returning an ambiguous code.
    for _ in 0..10 {
        let code = generate_join_code();
        let inserted = sqlx::query(
            "INSERT INTO join_codes \
             (id, swarm_id, created_by, code_hash, device_type_hint, expires_at, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(code_hash) WHERE redeemed_by_device IS NULL DO NOTHING",
        )
        .bind(new_id())
        .bind(&swarm_id)
        .bind(&user.user_id)
        .bind(token_hash(&code))
        .bind(device_type_hint)
        .bind(expires_at)
        .bind(now())
        .execute(&state.db)
        .await?;
        if inserted.rows_affected() == 1 {
            minted_code = Some(code);
            break;
        }
    }
    let code = minted_code.ok_or_else(|| AppError::internal("could not allocate a unique join code"))?;
    Ok((axum::http::StatusCode::CREATED, Json(JoinCodeResponse { code, expires_at: rfc3339(expires_at) })))
}

#[utoipa::path(get, path = "/api/v1/swarms/{swarm_id}/devices",
    responses((status = 200, body = SwarmDevicesResponse)), tag = "swarms")]
pub async fn devices(
    State(state): State<SharedState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(swarm_id): Path<String>,
) -> ApiResult<Json<SwarmDevicesResponse>> {
    // Two audiences: the owning user's browser session, or a device (Bearer)
    // that is itself a member of the swarm — the roster is what peers pin
    // fingerprints from.
    let swarm = if let Some(user) = session_user(&state, &jar).await? {
        owned_swarm(&state, &user.user_id, &swarm_id).await?
    } else {
        let device = require_device(&state, &headers).await?;
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT s.id, s.name FROM swarms s JOIN swarm_devices sd ON sd.swarm_id = s.id \
             WHERE s.id = ? AND sd.device_id = ?",
        )
        .bind(&swarm_id)
        .bind(&device.device_id)
        .fetch_optional(&state.db)
        .await?;
        row.map(|(id, name)| SwarmSummary { id, name })
            .ok_or_else(|| AppError::forbidden("device is not a member of this swarm"))?
    };

    let rows: Vec<(String, String, String, String, Option<i64>)> = sqlx::query_as(
        "SELECT d.id, d.name, d.device_type, d.cert_fingerprint, d.last_seen_at FROM devices d \
         JOIN swarm_devices sd ON sd.device_id = d.id \
         WHERE sd.swarm_id = ? AND d.revoked_at IS NULL ORDER BY d.created_at",
    )
    .bind(&swarm_id)
    .fetch_all(&state.db)
    .await?;
    let meta_rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT dm.device_id, dm.key, dm.value FROM device_metadata dm \
         JOIN swarm_devices sd ON sd.device_id = dm.device_id WHERE sd.swarm_id = ?",
    )
    .bind(&swarm_id)
    .fetch_all(&state.db)
    .await?;
    let mut metadata: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for (device_id, key, value) in meta_rows {
        metadata.entry(device_id).or_default().insert(key, value);
    }
    let devices = rows
        .into_iter()
        .map(|(device_id, name, device_type, cert_fingerprint, last_seen_at)| SwarmDevice {
            online: state.hub.is_online(&device_id),
            metadata: metadata.remove(&device_id).unwrap_or_default(),
            device_id,
            name,
            device_type: parse_device_type(&device_type),
            cert_fingerprint,
            last_seen_at: last_seen_at.map(rfc3339),
        })
        .collect();
    Ok(Json(SwarmDevicesResponse { swarm, devices }))
}

/// A device leaves one swarm it belongs to, without touching its other
/// memberships or its access token — symmetric with `devices::join_swarm`.
/// Self-only (mirrors `devices::patch_metadata`'s own-resource check): a
/// device cannot be kicked from a swarm by another device, only by the
/// swarm's owner deleting the whole swarm (`delete`, above).
#[utoipa::path(delete, path = "/api/v1/swarms/{swarm_id}/devices/{device_id}",
    responses((status = 200, body = super::auth::StatusResponse)), security(("bearerAuth" = [])), tag = "swarms")]
pub async fn leave(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path((swarm_id, device_id)): Path<(String, String)>,
) -> ApiResult<Json<super::auth::StatusResponse>> {
    let device = require_device(&state, &headers).await?;
    if device.device_id != device_id {
        return Err(AppError::forbidden("a device may only remove its own swarm membership"));
    }
    sqlx::query("DELETE FROM swarm_devices WHERE swarm_id = ? AND device_id = ?")
        .bind(&swarm_id)
        .bind(&device_id)
        .execute(&state.db)
        .await?;
    Ok(Json(super::auth::StatusResponse { status: "ok".into() }))
}
