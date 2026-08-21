//! Router assembly + OpenAPI document.

pub mod auth;
pub mod devices;
pub mod managed;
pub mod swarms;
pub mod ws;

use crate::state::SharedState;
use axum::routing::{delete, get, patch, post};
use axum::Router;
use tower_http::services::{ServeDir, ServeFile};
use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "SWARM Rendezvous API",
        description = "Rendezvous service for the SWARM P2P media suite: accounts, swarms, join codes, device registry, and WSS signaling (see /api/v1/ws — not representable in OpenAPI).",
        version = env!("CARGO_PKG_VERSION"),
    ),
    paths(
        auth::register, auth::login, auth::logout, auth::session, auth::change_password,
        auth::verify_email, auth::request_reset, auth::reset_password,
        swarms::list, swarms::create, swarms::delete, swarms::create_code, swarms::devices, swarms::leave,
        devices::register, devices::join_swarm, devices::my_devices, devices::revoke, devices::patch_metadata,
        managed::provision, managed::create_activation, managed::lookup_activation,
        managed::approve_activation, managed::activation_status,
    ),
    components(schemas(
        swarm_core::rest::DeviceType, swarm_core::rest::RegisterDeviceRequest,
        swarm_core::rest::DeviceRegistration, swarm_core::rest::RegisterDeviceResponse,
        swarm_core::rest::JoinSwarmRequest, swarm_core::rest::SwarmSummary,
        swarm_core::rest::SwarmDevice, swarm_core::rest::SwarmDevicesResponse,
        swarm_core::rest::MetadataPatchRequest, swarm_core::rest::ApiError,
        swarm_core::rest::ProvisionManagedSwarmRequest, swarm_core::rest::ProvisionManagedSwarmResponse,
        swarm_core::rest::CreateActivationRequest, swarm_core::rest::CreateActivationResponse,
        swarm_core::rest::LookupActivationRequest, swarm_core::rest::ActivationPreview,
        swarm_core::rest::ActivationStatus, swarm_core::rest::ActivationStatusResponse,
    )),
    modifiers(&SecuritySchemes),
    tags(
        (name = "auth", description = "Account registration and sessions (browser, cookie auth)"),
        (name = "swarms", description = "Swarm management and rosters"),
        (name = "devices", description = "Device registration and management"),
        (name = "managed swarms", description = "Media-server-owned automatic swarms"),
        (name = "activation", description = "Short-lived TV activation approved by a media server"),
    )
)]
pub struct ApiDoc;

struct SecuritySchemes;

impl Modify for SecuritySchemes {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearerAuth",
            SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build()),
        );
        components.add_security_scheme(
            "sessionCookie",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new(
                crate::authn::SESSION_COOKIE,
            ))),
        );
    }
}

pub fn build_router(state: SharedState, static_dir: Option<&str>) -> Router {
    let api = Router::new()
        .route("/auth/register", post(auth::register))
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/session", get(auth::session))
        .route("/auth/password", post(auth::change_password))
        .route("/auth/verify", post(auth::verify_email))
        .route("/auth/request-reset", post(auth::request_reset))
        .route("/auth/reset", post(auth::reset_password))
        .route("/swarms", get(swarms::list).post(swarms::create))
        .route("/swarms/join", post(devices::join_swarm))
        .route("/swarms/{swarm_id}", delete(swarms::delete))
        .route("/swarms/{swarm_id}/codes", post(swarms::create_code))
        .route("/swarms/{swarm_id}/devices", get(swarms::devices))
        .route(
            "/swarms/{swarm_id}/devices/{device_id}",
            delete(swarms::leave),
        )
        .route("/devices/register", post(devices::register))
        .route("/me/devices", get(devices::my_devices))
        .route("/devices/{device_id}", delete(devices::revoke))
        .route(
            "/devices/{device_id}/metadata",
            patch(devices::patch_metadata),
        )
        .route("/managed-swarms/provision", post(managed::provision))
        .route("/activations", post(managed::create_activation))
        .route("/activations/lookup", post(managed::lookup_activation))
        .route(
            "/activations/{activation_id}",
            get(managed::activation_status),
        )
        .route(
            "/activations/{activation_id}/approve",
            post(managed::approve_activation),
        )
        .route("/ws", get(ws::upgrade));

    let mut router = Router::new()
        .nest("/api/v1", api)
        .route("/health", get(|| async { "ok" }))
        .merge(
            utoipa_swagger_ui::SwaggerUi::new("/api/docs")
                .url("/api/openapi.json", ApiDoc::openapi()),
        );
    if let Some(dir) = static_dir {
        let index = ServeFile::new(format!("{dir}/index.html"));
        router = router.fallback_service(ServeDir::new(dir).fallback(index));
    }
    router.with_state(state)
}
