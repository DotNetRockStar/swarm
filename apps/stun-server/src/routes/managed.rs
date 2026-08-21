//! Media-server-owned swarms and short-lived TV activation.
//!
//! The rendezvous service stores routing state and hashed credentials. The
//! media server remains the invitation authority: a TV cannot become a swarm
//! member until the already-registered owner device approves its activation.

use crate::authn::require_device;
use crate::db::{now, rfc3339};
use crate::error::{ApiResult, AppError};
use crate::routes::devices::{add_membership, replace_metadata, validate_registration};
use crate::routes::swarms::device_type_str;
use crate::security::{generate_join_code, generate_token, new_id, token_hash};
use crate::state::SharedState;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use sqlx::SqliteConnection;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use swarm_core::rest::{
    ActivationPreview, ActivationStatus, ActivationStatusResponse, CreateActivationRequest,
    CreateActivationResponse, DeviceRegistration, DeviceType, LookupActivationRequest,
    ProvisionManagedSwarmRequest, ProvisionManagedSwarmResponse, SwarmSummary,
};

const MAX_MANAGED_SWARMS: i64 = 10_000;
const APPROVED_ACTIVATION_AUDIT_SECS: i64 = 30 * 24 * 3600;

#[derive(sqlx::FromRow)]
struct ActivationApprovalRow {
    device_name: String,
    device_type: String,
    machine_id: String,
    cert_fingerprint: String,
    platform: String,
    app_version: String,
    metadata_json: String,
    access_token_hash: String,
    requesting_device_id: Option<String>,
    status: String,
    expires_at: i64,
    approved_swarm_id: Option<String>,
    completed_device_id: Option<String>,
}

#[derive(sqlx::FromRow)]
struct ActivationStatusRow {
    status: String,
    expires_at: i64,
    completed_device_id: Option<String>,
    swarm_id: Option<String>,
    swarm_name: Option<String>,
}

fn validate_secret(value: &str, label: &'static str) -> Result<(), AppError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(AppError::bad_request(
            "invalid_credential",
            format!("{label} must be 64 lowercase hex characters"),
        ));
    }
    Ok(())
}

async fn owner_swarm(state: &SharedState, headers: &HeaderMap) -> ApiResult<(String, String)> {
    let device = require_device(state, headers).await?;
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT m.swarm_id, s.name FROM managed_swarm_owners m JOIN swarms s ON s.id = m.swarm_id \
         WHERE m.owner_device_id = ? AND m.lease_expires_at >= ?",
    )
    .bind(&device.device_id)
    .bind(now())
    .fetch_optional(&state.db)
    .await?;
    row.ok_or_else(|| AppError::forbidden("this device does not own an active managed swarm"))
}

async fn upsert_device(
    connection: &mut SqliteConnection,
    user_id: &str,
    device: &DeviceRegistration,
    access_token_hash: &str,
) -> ApiResult<String> {
    let ts = now();
    let existing: Option<(String,)> =
        sqlx::query_as("SELECT id FROM devices WHERE user_id = ? AND machine_id = ?")
            .bind(user_id)
            .bind(&device.machine_id)
            .fetch_optional(&mut *connection)
            .await?;
    let id = if let Some((id,)) = existing {
        sqlx::query(
            "UPDATE devices SET name = ?, device_type = ?, cert_fingerprint = ?, platform = ?, app_version = ?, \
             access_token_hash = ?, revoked_at = NULL, last_seen_at = ? WHERE id = ?",
        )
        .bind(device.name.trim())
        .bind(device_type_str(device.device_type))
        .bind(&device.cert_fingerprint)
        .bind(&device.platform)
        .bind(&device.app_version)
        .bind(access_token_hash)
        .bind(ts)
        .bind(&id)
        .execute(&mut *connection)
        .await?;
        id
    } else {
        let id = new_id();
        sqlx::query(
            "INSERT INTO devices (id, user_id, name, device_type, machine_id, cert_fingerprint, platform, app_version, \
             access_token_hash, last_seen_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(user_id)
        .bind(device.name.trim())
        .bind(device_type_str(device.device_type))
        .bind(&device.machine_id)
        .bind(&device.cert_fingerprint)
        .bind(&device.platform)
        .bind(&device.app_version)
        .bind(access_token_hash)
        .bind(ts)
        .bind(ts)
        .execute(&mut *connection)
        .await?;
        id
    };
    replace_metadata(connection, &id, &device.metadata).await?;
    Ok(id)
}

#[utoipa::path(post, path = "/api/v1/managed-swarms/provision", request_body = ProvisionManagedSwarmRequest,
    responses((status = 201, body = ProvisionManagedSwarmResponse)), tag = "managed swarms")]
pub async fn provision(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(req): Json<ProvisionManagedSwarmRequest>,
) -> ApiResult<(StatusCode, Json<ProvisionManagedSwarmResponse>)> {
    validate_registration(&req.device)?;
    validate_secret(&req.swarm_id, "swarm_id")?;
    validate_secret(&req.claim_token, "claim_token")?;
    if req.device.device_type == DeviceType::Client {
        return Err(AppError::bad_request(
            "invalid_device_type",
            "managed swarm owner must be a server",
        ));
    }
    let name = req.swarm_name.trim();
    if name.is_empty() || name.len() > 64 {
        return Err(AppError::bad_request(
            "invalid_name",
            "swarm name must be 1-64 characters",
        ));
    }

    let claim_hash = token_hash(&req.claim_token);
    let existing: Option<(String, String, String)> = sqlx::query_as(
        "SELECT m.owner_device_id, d.machine_id, d.cert_fingerprint FROM managed_swarm_owners m \
         JOIN devices d ON d.id = m.owner_device_id WHERE m.swarm_id = ? AND m.claim_token_hash = ?",
    )
    .bind(&req.swarm_id)
    .bind(&claim_hash)
    .fetch_optional(&state.db)
    .await?;
    let is_new = existing.is_none();
    if is_new {
        // Reclaim the complete synthetic owner graph for expired managed
        // swarms. Deleting the user cascades its swarm, devices, metadata,
        // memberships and owner row, so leases actually bound storage.
        sqlx::query(
            "DELETE FROM users WHERE id IN (SELECT s.owner_user_id FROM swarms s \
             JOIN managed_swarm_owners m ON m.swarm_id = s.id WHERE m.lease_expires_at < ?)",
        )
        .bind(now())
        .execute(&state.db)
        .await?;
        let managed_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM managed_swarm_owners")
            .fetch_one(&state.db)
            .await?;
        if managed_count >= MAX_MANAGED_SWARMS {
            return Err(AppError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "capacity",
                "managed swarm capacity is currently full",
            ));
        }
        let occupied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM swarms WHERE id = ?")
            .bind(&req.swarm_id)
            .fetch_one(&state.db)
            .await?;
        if occupied != 0 {
            state.blocker.record_failure(addr.ip());
            return Err(AppError::forbidden("managed swarm claim does not match"));
        }
        if !state.managed_swarm_allocations.allow(addr.ip()) {
            return Err(AppError::too_many_requests());
        }
    } else if let Some((_, machine_id, fingerprint)) = &existing {
        if machine_id != &req.device.machine_id || fingerprint != &req.device.cert_fingerprint {
            state.blocker.record_failure(addr.ip());
            return Err(AppError::forbidden(
                "managed swarm is bound to a different server identity",
            ));
        }
    }

    let access_token = generate_token();
    let access_hash = token_hash(&access_token);
    let ts = now();
    let lease = ts + state.config.managed_swarm_lease_secs;
    let mut tx = state.db.begin().await?;
    let (user_id, status) = if is_new {
        let user_id = new_id();
        sqlx::query("INSERT INTO users (id, email, password_hash, email_verified_at, created_at) VALUES (?, ?, ?, ?, ?)")
            .bind(&user_id)
            .bind(format!("managed+{}@internal.invalid", req.swarm_id))
            .bind(token_hash(&generate_token()))
            .bind(ts)
            .bind(ts)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO swarms (id, owner_user_id, name, created_at) VALUES (?, ?, ?, ?)")
            .bind(&req.swarm_id)
            .bind(&user_id)
            .bind(name)
            .bind(ts)
            .execute(&mut *tx)
            .await?;
        (user_id, StatusCode::CREATED)
    } else {
        let user_id: String = sqlx::query_scalar("SELECT owner_user_id FROM swarms WHERE id = ?")
            .bind(&req.swarm_id)
            .fetch_one(&mut *tx)
            .await?;
        sqlx::query("UPDATE swarms SET name = ? WHERE id = ?")
            .bind(name)
            .bind(&req.swarm_id)
            .execute(&mut *tx)
            .await?;
        (user_id, StatusCode::OK)
    };
    let device_id = upsert_device(&mut tx, &user_id, &req.device, &access_hash).await?;
    add_membership(&mut tx, &req.swarm_id, &device_id).await?;
    sqlx::query(
        "INSERT INTO managed_swarm_owners (swarm_id, owner_device_id, claim_token_hash, lease_expires_at, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(swarm_id) DO UPDATE SET lease_expires_at = excluded.lease_expires_at, \
         updated_at = excluded.updated_at",
    )
    .bind(&req.swarm_id).bind(&device_id).bind(&claim_hash).bind(lease).bind(ts).bind(ts)
    .execute(&mut *tx).await?;
    tx.commit().await?;
    state.blocker.record_success(addr.ip());
    Ok((
        status,
        Json(ProvisionManagedSwarmResponse {
            access_token,
            device_id,
            swarm: SwarmSummary {
                id: req.swarm_id,
                name: name.to_string(),
            },
            lease_expires_at: rfc3339(lease),
        }),
    ))
}

#[utoipa::path(post, path = "/api/v1/activations", request_body = CreateActivationRequest,
    responses((status = 201, body = CreateActivationResponse)), tag = "activation")]
pub async fn create_activation(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<CreateActivationRequest>,
) -> ApiResult<(StatusCode, Json<CreateActivationResponse>)> {
    validate_registration(&req.device)?;
    if req.device.device_type == DeviceType::Server {
        return Err(AppError::bad_request(
            "invalid_device_type",
            "activation is for client devices",
        ));
    }
    if !state.activation_allocations.allow(addr.ip()) {
        return Err(AppError::too_many_requests());
    }
    let ts = now();
    sqlx::query(
        "DELETE FROM device_activations WHERE (expires_at < ? AND status = 'pending') \
         OR (status = 'approved' AND approved_at < ?)",
    )
    .bind(ts)
    .bind(ts - APPROVED_ACTIVATION_AUDIT_SECS)
    .execute(&state.db)
    .await?;
    let id = new_id();
    let poll_token = generate_token();
    let access_token = generate_token();
    let expires = ts + state.config.activation_ttl_secs;
    let metadata_json = serde_json::to_string(&req.device.metadata)
        .map_err(|_| AppError::bad_request("invalid_metadata", "metadata could not be encoded"))?;
    let requesting_device_id = if headers.contains_key(axum::http::header::AUTHORIZATION) {
        Some(require_device(&state, &headers).await?.device_id)
    } else {
        None
    };

    let mut code = String::new();
    let mut inserted = false;
    for _ in 0..8 {
        code = generate_join_code();
        let result = sqlx::query(
            "INSERT INTO device_activations (id, code_hash, poll_token_hash, access_token_hash, requesting_device_id, device_name, device_type, \
             machine_id, cert_fingerprint, platform, app_version, metadata_json, status, expires_at, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?)",
        )
        .bind(&id).bind(token_hash(&code)).bind(token_hash(&poll_token)).bind(token_hash(&access_token)).bind(&requesting_device_id)
        .bind(req.device.name.trim()).bind(device_type_str(req.device.device_type)).bind(&req.device.machine_id)
        .bind(&req.device.cert_fingerprint).bind(&req.device.platform).bind(&req.device.app_version)
        .bind(&metadata_json).bind(expires).bind(ts).execute(&state.db).await;
        match result {
            Ok(_) => {
                inserted = true;
                break;
            }
            Err(sqlx::Error::Database(db)) if db.is_unique_violation() => continue,
            Err(err) => return Err(err.into()),
        }
    }
    if !inserted {
        return Err(AppError::internal(
            "could not allocate a unique activation code",
        ));
    }
    Ok((
        StatusCode::CREATED,
        Json(CreateActivationResponse {
            activation_id: id,
            code,
            poll_token,
            access_token,
            expires_at: rfc3339(expires),
        }),
    ))
}

#[utoipa::path(post, path = "/api/v1/activations/lookup", request_body = LookupActivationRequest,
    responses((status = 200, body = ActivationPreview)), security(("bearerAuth" = [])), tag = "activation")]
pub async fn lookup_activation(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<LookupActivationRequest>,
) -> ApiResult<Json<ActivationPreview>> {
    if state.blocker.is_blocked(addr.ip()) {
        return Err(AppError::too_many_requests());
    }
    owner_swarm(&state, &headers).await?;
    let row: Option<(String, String, String, i64)> = sqlx::query_as(
        "SELECT id, device_name, platform, expires_at FROM device_activations \
         WHERE code_hash = ? AND status = 'pending' AND expires_at >= ?",
    )
    .bind(token_hash(req.code.trim()))
    .bind(now())
    .fetch_optional(&state.db)
    .await?;
    let Some((activation_id, device_name, platform, expires_at)) = row else {
        state.blocker.record_failure(addr.ip());
        return Err(AppError::not_found(
            "activation code is invalid, expired, or already used",
        ));
    };
    state.blocker.record_success(addr.ip());
    Ok(Json(ActivationPreview {
        activation_id,
        device_name,
        platform,
        expires_at: rfc3339(expires_at),
    }))
}

#[utoipa::path(post, path = "/api/v1/activations/{activation_id}/approve",
    responses((status = 200, body = ActivationStatusResponse)), security(("bearerAuth" = [])), tag = "activation")]
pub async fn approve_activation(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(activation_id): Path<String>,
) -> ApiResult<Json<ActivationStatusResponse>> {
    let (swarm_id, swarm_name) = owner_swarm(&state, &headers).await?;
    let mut tx = state.db.begin().await?;
    let row: Option<ActivationApprovalRow> = sqlx::query_as(
        "SELECT device_name, device_type, machine_id, cert_fingerprint, platform, app_version, metadata_json, access_token_hash, requesting_device_id, \
         status, expires_at, approved_swarm_id, completed_device_id FROM device_activations WHERE id = ?",
    ).bind(&activation_id).fetch_optional(&mut *tx).await?;
    let Some(ActivationApprovalRow {
        device_name: name,
        device_type,
        machine_id,
        cert_fingerprint,
        platform,
        app_version,
        metadata_json,
        access_token_hash: access_hash,
        requesting_device_id,
        status,
        expires_at: expires,
        approved_swarm_id: approved_swarm,
        completed_device_id: completed_device,
    }) = row
    else {
        return Err(AppError::not_found("activation not found"));
    };
    if status == "approved" {
        if approved_swarm.as_deref() != Some(&swarm_id) {
            return Err(AppError::forbidden(
                "activation was approved by another swarm",
            ));
        }
        return Ok(Json(ActivationStatusResponse {
            status: ActivationStatus::Approved,
            device_id: completed_device,
            swarm: Some(SwarmSummary {
                id: swarm_id,
                name: swarm_name,
            }),
            expires_at: rfc3339(expires),
        }));
    }
    if expires < now() {
        return Err(AppError::not_found("activation has expired"));
    }
    let client_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM swarm_devices sd JOIN devices d ON d.id = sd.device_id \
         WHERE sd.swarm_id = ? AND d.device_type IN ('client','both') AND d.revoked_at IS NULL",
    )
    .bind(&swarm_id)
    .fetch_one(&mut *tx)
    .await?;
    if client_count >= state.config.managed_swarm_max_clients {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "client_limit",
            "this swarm has reached its client limit",
        ));
    }
    let device_id = if let Some(requesting_device_id) = requesting_device_id {
        let fingerprint: Option<String> = sqlx::query_scalar(
            "SELECT cert_fingerprint FROM devices WHERE id = ? AND revoked_at IS NULL",
        )
        .bind(&requesting_device_id)
        .fetch_optional(&mut *tx)
        .await?;
        if fingerprint.as_deref() != Some(cert_fingerprint.as_str()) {
            return Err(AppError::forbidden(
                "activation identity no longer matches the requesting device",
            ));
        }
        requesting_device_id
    } else {
        let user_id: String = sqlx::query_scalar("SELECT owner_user_id FROM swarms WHERE id = ?")
            .bind(&swarm_id)
            .fetch_one(&mut *tx)
            .await?;
        let metadata: BTreeMap<String, String> = serde_json::from_str(&metadata_json)
            .map_err(|_| AppError::internal("stored activation metadata is invalid"))?;
        let registration = DeviceRegistration {
            name,
            device_type: if device_type == "both" {
                DeviceType::Both
            } else {
                DeviceType::Client
            },
            machine_id,
            cert_fingerprint,
            platform,
            app_version,
            metadata,
        };
        upsert_device(&mut tx, &user_id, &registration, &access_hash).await?
    };
    add_membership(&mut tx, &swarm_id, &device_id).await?;
    let changed = sqlx::query(
        "UPDATE device_activations SET status = 'approved', approved_swarm_id = ?, completed_device_id = ?, approved_at = ? \
         WHERE id = ? AND status = 'pending'",
    ).bind(&swarm_id).bind(&device_id).bind(now()).bind(&activation_id).execute(&mut *tx).await?;
    if changed.rows_affected() != 1 {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "activation_used",
            "activation was already used",
        ));
    }
    sqlx::query(
        "UPDATE managed_swarm_owners SET lease_expires_at = ?, updated_at = ? WHERE swarm_id = ?",
    )
    .bind(now() + state.config.managed_swarm_lease_secs)
    .bind(now())
    .bind(&swarm_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(ActivationStatusResponse {
        status: ActivationStatus::Approved,
        device_id: Some(device_id),
        swarm: Some(SwarmSummary {
            id: swarm_id,
            name: swarm_name,
        }),
        expires_at: rfc3339(expires),
    }))
}

#[utoipa::path(get, path = "/api/v1/activations/{activation_id}",
    responses((status = 200, body = ActivationStatusResponse)), security(("bearerAuth" = [])), tag = "activation")]
pub async fn activation_status(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(activation_id): Path<String>,
) -> ApiResult<Json<ActivationStatusResponse>> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| AppError::unauthorized("poll token required"))?;
    let row: Option<ActivationStatusRow> = sqlx::query_as(
        "SELECT a.status, a.expires_at, a.completed_device_id, s.id AS swarm_id, s.name AS swarm_name FROM device_activations a \
         LEFT JOIN swarms s ON s.id = a.approved_swarm_id WHERE a.id = ? AND a.poll_token_hash = ?",
    ).bind(&activation_id).bind(token_hash(token)).fetch_optional(&state.db).await?;
    let Some(ActivationStatusRow {
        status,
        expires_at: expires,
        completed_device_id: device_id,
        swarm_id,
        swarm_name,
    }) = row
    else {
        return Err(AppError::unauthorized("unknown activation or poll token"));
    };
    let (status, swarm) = if status == "approved" {
        (
            ActivationStatus::Approved,
            Some(SwarmSummary {
                id: swarm_id.unwrap_or_default(),
                name: swarm_name.unwrap_or_default(),
            }),
        )
    } else if expires < now() {
        (ActivationStatus::Expired, None)
    } else {
        (ActivationStatus::Pending, None)
    };
    Ok(Json(ActivationStatusResponse {
        status,
        device_id,
        swarm,
        expires_at: rfc3339(expires),
    }))
}
