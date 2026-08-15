//! Request authentication helpers: cookie sessions for the browser, Bearer
//! access tokens for devices, and CSRF enforcement for mutating browser
//! routes.
//!
//! Session model (ported from Batocera.Drone `auth.py`): opaque token in an
//! HttpOnly SameSite=Strict cookie, stored hashed with a sliding 30-day
//! expiry; the expiry write is throttled to once per 6h so hot sessions don't
//! write on every request. CSRF is double-submit: a non-HttpOnly `swarm_csrf`
//! cookie whose value mutating requests must echo in the `x-swarm-csrf`
//! header (SameSite=Strict already blocks cross-origin sends; the header
//! check backstops it).

use crate::db::now;
use crate::error::AppError;
use crate::security::token_hash;
use crate::state::SharedState;
use axum::http::HeaderMap;
use axum_extra::extract::cookie::CookieJar;

pub const SESSION_COOKIE: &str = "swarm_session";
pub const CSRF_COOKIE: &str = "swarm_csrf";
pub const CSRF_HEADER: &str = "x-swarm-csrf";
const TOUCH_THROTTLE_SECS: i64 = 6 * 3600;

pub struct SessionUser {
    pub user_id: String,
    pub email: String,
}

/// Resolve the session cookie to a user, sliding the expiry forward
/// (throttled). Returns None for missing/expired/unknown sessions.
pub async fn session_user(state: &SharedState, jar: &CookieJar) -> Result<Option<SessionUser>, AppError> {
    let Some(cookie) = jar.get(SESSION_COOKIE) else {
        return Ok(None);
    };
    let hash = token_hash(cookie.value());
    let ts = now();
    let row: Option<(String, String, i64, i64)> = sqlx::query_as(
        "SELECT u.id, u.email, s.expires_at, s.last_seen_at FROM sessions s \
         JOIN users u ON u.id = s.user_id WHERE s.token_hash = ?",
    )
    .bind(&hash)
    .fetch_optional(&state.db)
    .await?;
    let Some((user_id, email, expires_at, last_seen_at)) = row else {
        return Ok(None);
    };
    if expires_at < ts {
        sqlx::query("DELETE FROM sessions WHERE token_hash = ?").bind(&hash).execute(&state.db).await?;
        return Ok(None);
    }
    if ts - last_seen_at > TOUCH_THROTTLE_SECS {
        sqlx::query("UPDATE sessions SET last_seen_at = ?, expires_at = ? WHERE token_hash = ?")
            .bind(ts)
            .bind(ts + state.config.session_ttl_secs)
            .bind(&hash)
            .execute(&state.db)
            .await?;
    }
    Ok(Some(SessionUser { user_id, email }))
}

pub async fn require_session(state: &SharedState, jar: &CookieJar) -> Result<SessionUser, AppError> {
    session_user(state, jar).await?.ok_or_else(|| AppError::unauthorized("login required"))
}

/// Double-submit CSRF check for mutating session-authenticated routes.
/// Device Bearer routes are exempt (no ambient cookie credential to ride).
pub fn require_csrf(jar: &CookieJar, headers: &HeaderMap) -> Result<(), AppError> {
    let cookie = jar.get(CSRF_COOKIE).map(|c| c.value().to_string());
    let header = headers.get(CSRF_HEADER).and_then(|v| v.to_str().ok()).map(str::to_string);
    match (cookie, header) {
        (Some(c), Some(h)) if !c.is_empty() && c == h => Ok(()),
        _ => Err(AppError::forbidden("missing or mismatched CSRF token")),
    }
}

pub struct DeviceAuth {
    pub device_id: String,
}

/// Resolve `Authorization: Bearer <access_token>` to a non-revoked device,
/// updating its last-seen timestamp.
pub async fn require_device(state: &SharedState, headers: &HeaderMap) -> Result<DeviceAuth, AppError> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| AppError::unauthorized("bearer token required"))?;
    let hash = token_hash(token);
    let row: Option<(String, Option<i64>)> = sqlx::query_as(
        "SELECT id, revoked_at FROM devices WHERE access_token_hash = ?",
    )
    .bind(&hash)
    .fetch_optional(&state.db)
    .await?;
    match row {
        Some((device_id, None)) => {
            sqlx::query("UPDATE devices SET last_seen_at = ? WHERE id = ?")
                .bind(now())
                .bind(&device_id)
                .execute(&state.db)
                .await?;
            Ok(DeviceAuth { device_id })
        }
        Some((_, Some(_))) => Err(AppError::unauthorized("device revoked")),
        None => Err(AppError::unauthorized("unknown access token")),
    }
}
