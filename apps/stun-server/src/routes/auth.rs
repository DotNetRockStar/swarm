//! Account routes: register, login/logout, session introspection, password
//! change, email verification, and password reset.
//!
//! Email delivery is not wired yet: verification/reset tokens are minted and
//! logged (visible in server logs / dev console). Accounts are usable before
//! verification; the flag only gates a "verified" badge for now.

use crate::authn::{require_csrf, require_session, session_user, CSRF_COOKIE, SESSION_COOKIE};
use crate::db::now;
use crate::error::{ApiResult, AppError};
use crate::security::{
    generate_token, hash_password, new_id, token_hash, validate_password, verify_password,
};
use crate::state::SharedState;
use axum::extract::{ConnectInfo, State};
use axum::http::HeaderMap;
use axum::Json;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RegisterUserRequest {
    pub email: String,
    pub password: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Serialize, ToSchema)]
pub struct SessionResponse {
    pub authenticated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email_verified: Option<bool>,
}

#[derive(Serialize, ToSchema)]
pub struct StatusResponse {
    pub status: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PasswordChangeRequest {
    pub current_password: String,
    pub new_password: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VerifyEmailRequest {
    pub token: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RequestResetRequest {
    pub email: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ResetPasswordRequest {
    pub token: String,
    pub new_password: String,
}

fn ok() -> Json<StatusResponse> {
    Json(StatusResponse { status: "ok".into() })
}

fn valid_email(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else { return false };
    !local.is_empty() && domain.contains('.') && !domain.ends_with('.') && email.len() <= 254
}

fn session_cookies(state: &SharedState, session_token: &str, csrf_token: &str) -> (Cookie<'static>, Cookie<'static>) {
    let secure = state.config.public_url.starts_with("https://");
    let session = Cookie::build((SESSION_COOKIE, session_token.to_string()))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Strict)
        .secure(secure)
        .build();
    // Deliberately not HttpOnly — the UI JS reads it to echo in x-swarm-csrf.
    let csrf = Cookie::build((CSRF_COOKIE, csrf_token.to_string()))
        .path("/")
        .same_site(SameSite::Strict)
        .secure(secure)
        .build();
    (session, csrf)
}

async fn create_session(state: &SharedState, user_id: &str) -> ApiResult<(String, String)> {
    let token = generate_token();
    let csrf = generate_token();
    let ts = now();
    // Opportunistic sweep of expired rows (auth.py pattern).
    sqlx::query("DELETE FROM sessions WHERE expires_at < ?").bind(ts).execute(&state.db).await?;
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, created_at, last_seen_at, expires_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(token_hash(&token))
    .bind(user_id)
    .bind(ts)
    .bind(ts)
    .bind(ts + state.config.session_ttl_secs)
    .execute(&state.db)
    .await?;
    Ok((token, csrf))
}

#[utoipa::path(post, path = "/api/v1/auth/register", request_body = RegisterUserRequest,
    responses((status = 201, body = StatusResponse), (status = 400, body = swarm_core::rest::ApiError)), tag = "auth")]
pub async fn register(
    State(state): State<SharedState>,
    Json(req): Json<RegisterUserRequest>,
) -> ApiResult<(axum::http::StatusCode, Json<StatusResponse>)> {
    let email = req.email.trim().to_lowercase();
    if !valid_email(&email) {
        return Err(AppError::bad_request("invalid_email", "enter a valid email address"));
    }
    validate_password(&req.password).map_err(|m| AppError::bad_request("weak_password", m))?;
    let password_hash = hash_password(&req.password).map_err(|_| AppError::internal("hashing failed"))?;
    let user_id = new_id();
    let inserted = sqlx::query(
        "INSERT INTO users (id, email, password_hash, created_at) VALUES (?, ?, ?, ?) ON CONFLICT(email) DO NOTHING",
    )
    .bind(&user_id)
    .bind(&email)
    .bind(&password_hash)
    .bind(now())
    .execute(&state.db)
    .await?;
    if inserted.rows_affected() == 0 {
        // Same response as success — don't leak which emails exist.
        return Ok((axum::http::StatusCode::CREATED, ok()));
    }
    let verify_token = generate_token();
    sqlx::query("INSERT INTO email_tokens (token_hash, user_id, purpose, expires_at) VALUES (?, ?, 'verify', ?)")
        .bind(token_hash(&verify_token))
        .bind(&user_id)
        .bind(now() + 24 * 3600)
        .execute(&state.db)
        .await?;
    tracing::info!(email, verify_url = %format!("{}/#verify={verify_token}", state.config.public_url),
        "verification link (email delivery not configured)");
    Ok((axum::http::StatusCode::CREATED, ok()))
}

#[utoipa::path(post, path = "/api/v1/auth/login", request_body = LoginRequest,
    responses((status = 200, body = SessionResponse), (status = 401, body = swarm_core::rest::ApiError)), tag = "auth")]
pub async fn login(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    jar: CookieJar,
    Json(req): Json<LoginRequest>,
) -> ApiResult<(CookieJar, Json<SessionResponse>)> {
    if state.blocker.is_blocked(addr.ip()) {
        return Err(AppError::too_many_requests());
    }
    let email = req.email.trim().to_lowercase();
    let row: Option<(String, String, Option<i64>)> =
        sqlx::query_as("SELECT id, password_hash, email_verified_at FROM users WHERE email = ?")
            .bind(&email)
            .fetch_optional(&state.db)
            .await?;
    let Some((user_id, password_hash, verified_at)) = row else {
        // Burn comparable time so unknown emails aren't distinguishable.
        let _ = verify_password(&req.password, "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        state.blocker.record_failure(addr.ip());
        return Err(AppError::unauthorized("invalid email or password"));
    };
    if !verify_password(&req.password, &password_hash) {
        state.blocker.record_failure(addr.ip());
        return Err(AppError::unauthorized("invalid email or password"));
    }
    state.blocker.record_success(addr.ip());
    let (session_token, csrf_token) = create_session(&state, &user_id).await?;
    let (session_cookie, csrf_cookie) = session_cookies(&state, &session_token, &csrf_token);
    Ok((
        jar.add(session_cookie).add(csrf_cookie),
        Json(SessionResponse { authenticated: true, email: Some(email), email_verified: Some(verified_at.is_some()) }),
    ))
}

#[utoipa::path(get, path = "/api/v1/auth/session", responses((status = 200, body = SessionResponse)), tag = "auth")]
pub async fn session(State(state): State<SharedState>, jar: CookieJar) -> ApiResult<Json<SessionResponse>> {
    match session_user(&state, &jar).await? {
        Some(user) => {
            let verified: Option<(Option<i64>,)> =
                sqlx::query_as("SELECT email_verified_at FROM users WHERE id = ?")
                    .bind(&user.user_id)
                    .fetch_optional(&state.db)
                    .await?;
            Ok(Json(SessionResponse {
                authenticated: true,
                email: Some(user.email),
                email_verified: Some(verified.and_then(|(v,)| v).is_some()),
            }))
        }
        None => Ok(Json(SessionResponse { authenticated: false, email: None, email_verified: None })),
    }
}

#[utoipa::path(post, path = "/api/v1/auth/logout", responses((status = 200, body = StatusResponse)), tag = "auth")]
pub async fn logout(State(state): State<SharedState>, jar: CookieJar) -> ApiResult<(CookieJar, Json<StatusResponse>)> {
    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
            .bind(token_hash(cookie.value()))
            .execute(&state.db)
            .await?;
    }
    Ok((jar.remove(Cookie::from(SESSION_COOKIE)).remove(Cookie::from(CSRF_COOKIE)), ok()))
}

#[utoipa::path(post, path = "/api/v1/auth/password", request_body = PasswordChangeRequest,
    responses((status = 200, body = StatusResponse)), tag = "auth")]
pub async fn change_password(
    State(state): State<SharedState>,
    jar: CookieJar,
    headers: HeaderMap,
    Json(req): Json<PasswordChangeRequest>,
) -> ApiResult<Json<StatusResponse>> {
    require_csrf(&jar, &headers)?;
    let user = require_session(&state, &jar).await?;
    let (stored,): (String,) = sqlx::query_as("SELECT password_hash FROM users WHERE id = ?")
        .bind(&user.user_id)
        .fetch_one(&state.db)
        .await?;
    if !verify_password(&req.current_password, &stored) {
        return Err(AppError::unauthorized("current password is incorrect"));
    }
    validate_password(&req.new_password).map_err(|m| AppError::bad_request("weak_password", m))?;
    let new_hash = hash_password(&req.new_password).map_err(|_| AppError::internal("hashing failed"))?;
    sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(&new_hash)
        .bind(&user.user_id)
        .execute(&state.db)
        .await?;
    // Sign out every other session but keep the caller's (auth.py pattern).
    let keep = jar.get(SESSION_COOKIE).map(|c| token_hash(c.value())).unwrap_or_default();
    sqlx::query("DELETE FROM sessions WHERE user_id = ? AND token_hash != ?")
        .bind(&user.user_id)
        .bind(&keep)
        .execute(&state.db)
        .await?;
    Ok(ok())
}

#[utoipa::path(post, path = "/api/v1/auth/verify", request_body = VerifyEmailRequest,
    responses((status = 200, body = StatusResponse)), tag = "auth")]
pub async fn verify_email(
    State(state): State<SharedState>,
    Json(req): Json<VerifyEmailRequest>,
) -> ApiResult<Json<StatusResponse>> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT user_id FROM email_tokens WHERE token_hash = ? AND purpose = 'verify' AND expires_at >= ?",
    )
    .bind(token_hash(&req.token))
    .bind(now())
    .fetch_optional(&state.db)
    .await?;
    let Some((user_id,)) = row else {
        return Err(AppError::bad_request("invalid_token", "verification link is invalid or expired"));
    };
    sqlx::query("UPDATE users SET email_verified_at = ? WHERE id = ?")
        .bind(now())
        .bind(&user_id)
        .execute(&state.db)
        .await?;
    sqlx::query("DELETE FROM email_tokens WHERE user_id = ? AND purpose = 'verify'")
        .bind(&user_id)
        .execute(&state.db)
        .await?;
    Ok(ok())
}

#[utoipa::path(post, path = "/api/v1/auth/request-reset", request_body = RequestResetRequest,
    responses((status = 200, body = StatusResponse)), tag = "auth")]
pub async fn request_reset(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(req): Json<RequestResetRequest>,
) -> ApiResult<Json<StatusResponse>> {
    if state.blocker.is_blocked(addr.ip()) {
        return Err(AppError::too_many_requests());
    }
    let email = req.email.trim().to_lowercase();
    let row: Option<(String,)> = sqlx::query_as("SELECT id FROM users WHERE email = ?")
        .bind(&email)
        .fetch_optional(&state.db)
        .await?;
    // Always answer ok — never reveal whether the email exists.
    if let Some((user_id,)) = row {
        let reset_token = generate_token();
        sqlx::query("INSERT INTO email_tokens (token_hash, user_id, purpose, expires_at) VALUES (?, ?, 'reset', ?)")
            .bind(token_hash(&reset_token))
            .bind(&user_id)
            .bind(now() + 3600)
            .execute(&state.db)
            .await?;
        tracing::info!(email, reset_url = %format!("{}/#reset={reset_token}", state.config.public_url),
            "password reset link (email delivery not configured)");
    }
    Ok(ok())
}

#[utoipa::path(post, path = "/api/v1/auth/reset", request_body = ResetPasswordRequest,
    responses((status = 200, body = StatusResponse)), tag = "auth")]
pub async fn reset_password(
    State(state): State<SharedState>,
    Json(req): Json<ResetPasswordRequest>,
) -> ApiResult<Json<StatusResponse>> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT user_id FROM email_tokens WHERE token_hash = ? AND purpose = 'reset' AND expires_at >= ?",
    )
    .bind(token_hash(&req.token))
    .bind(now())
    .fetch_optional(&state.db)
    .await?;
    let Some((user_id,)) = row else {
        return Err(AppError::bad_request("invalid_token", "reset link is invalid or expired"));
    };
    validate_password(&req.new_password).map_err(|m| AppError::bad_request("weak_password", m))?;
    let new_hash = hash_password(&req.new_password).map_err(|_| AppError::internal("hashing failed"))?;
    sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(&new_hash)
        .bind(&user_id)
        .execute(&state.db)
        .await?;
    // Reset invalidates every session and outstanding reset token.
    sqlx::query("DELETE FROM sessions WHERE user_id = ?").bind(&user_id).execute(&state.db).await?;
    sqlx::query("DELETE FROM email_tokens WHERE user_id = ? AND purpose = 'reset'")
        .bind(&user_id)
        .execute(&state.db)
        .await?;
    Ok(ok())
}
