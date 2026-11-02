use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use sea_orm::DatabaseConnection;
use serde_json::json;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use webdock_core::config::PlatformConfig;

pub struct ServiceState {
    pub realtime: Option<crate::realtime::Realtime>,
    pub resource_client: Option<crate::resource_auth::ResourceClientConfig>,
    pub config: PlatformConfig,
    pub db: DatabaseConnection,
    pub identity: Option<Arc<webdock_identity::IdentityService>>,
    pub registry: Option<Arc<webdock_registry::RegistryService>>,
    pub oauth: Option<Arc<webdock_oauth::OAuthService>>,
    pub hosting: Option<Arc<webdock_hosting::HostingService>>,
    pub build: Option<Arc<webdock_build::BuildService>>,
    pub plugins: Option<Arc<crate::plugins::PluginEndpoints>>,
    pub databases: Option<Arc<crate::databases::DatabaseEndpoints>>,
    pub mail: Option<Arc<crate::mail::MailEndpoints>>,
    pub neon_connections: Option<Arc<crate::neon_connections::NeonConnections>>,
    pub browser: Option<Arc<webdock_browser_session::BrowserSessions>>,
    pub access: Option<Arc<webdock_access::AccessService>>,
    pub ui: Option<Arc<crate::ui::UiAssets>>,
    pub auth_limiter: governor::DefaultDirectRateLimiter,
}

#[derive(Default)]
pub struct Services {
    pub realtime: Option<crate::realtime::Realtime>,
    pub resource_client: Option<crate::resource_auth::ResourceClientConfig>,
    pub identity: Option<Arc<webdock_identity::IdentityService>>,
    pub registry: Option<Arc<webdock_registry::RegistryService>>,
    pub oauth: Option<Arc<webdock_oauth::OAuthService>>,
    pub hosting: Option<Arc<webdock_hosting::HostingService>>,
    pub build: Option<Arc<webdock_build::BuildService>>,
    pub plugins: Option<Arc<crate::plugins::PluginEndpoints>>,
    pub databases: Option<Arc<crate::databases::DatabaseEndpoints>>,
    pub mail: Option<Arc<crate::mail::MailEndpoints>>,
    pub neon_connections: Option<Arc<crate::neon_connections::NeonConnections>>,
    pub browser: Option<Arc<webdock_browser_session::BrowserSessions>>,
    pub access: Option<Arc<webdock_access::AccessService>>,
    pub ui: Option<Arc<crate::ui::UiAssets>>,
}

pub fn build_app(config: PlatformConfig, db: DatabaseConnection) -> Router {
    build_app_with_identity(config, db, None)
}

pub fn build_app_with_identity(
    config: PlatformConfig,
    db: DatabaseConnection,
    identity: Option<Arc<webdock_identity::IdentityService>>,
) -> Router {
    build_app_with_services(config, db, identity, None)
}

pub fn build_app_with_services(
    config: PlatformConfig,
    db: DatabaseConnection,
    identity: Option<Arc<webdock_identity::IdentityService>>,
    registry: Option<Arc<webdock_registry::RegistryService>>,
) -> Router {
    build_app_with_oauth(config, db, identity, registry, None)
}

pub fn build_app_with_oauth(
    config: PlatformConfig,
    db: DatabaseConnection,
    identity: Option<Arc<webdock_identity::IdentityService>>,
    registry: Option<Arc<webdock_registry::RegistryService>>,
    oauth: Option<Arc<webdock_oauth::OAuthService>>,
) -> Router {
    build_app_with_domains(
        config,
        db,
        Services {
            identity,
            registry,
            oauth,
            ..Default::default()
        },
    )
}

pub fn build_app_with_domains(
    config: PlatformConfig,
    db: DatabaseConnection,
    services: Services,
) -> Router {
    let origins: Vec<HeaderValue> = config
        .allowed_origins()
        .iter()
        .filter_map(|value| value.parse().ok())
        .collect();
    let worker_routes =
        crate::build::worker_routes(services.build.clone(), config.features.clone());
    let state = Arc::new(ServiceState {
        realtime: services.realtime,
        resource_client: services.resource_client,
        config,
        db,
        identity: services.identity,
        registry: services.registry,
        oauth: services.oauth,
        hosting: services.hosting,
        build: services.build,
        plugins: services.plugins,
        databases: services.databases,
        mail: services.mail,
        neon_connections: services.neon_connections,
        browser: services.browser,
        access: services.access,
        ui: services.ui,
        auth_limiter: governor::RateLimiter::direct(
            governor::Quota::per_second(std::num::NonZeroU32::new(5).unwrap())
                .allow_burst(std::num::NonZeroU32::new(20).unwrap()),
        ),
    });
    Router::new()
        .merge(crate::auth::routes())
        .merge(crate::realtime::routes())
        .merge(crate::oauth::routes())
        .merge(crate::browser::routes())
        .merge(crate::access::routes())
        .merge(crate::waitlist::routes())
        .merge(crate::registry::routes())
        .merge(crate::resource_registry::routes())
        .merge(crate::resource_hosting::routes())
        .merge(crate::hosting::routes())
        .merge(crate::build_webhook::routes())
        .merge(crate::plugins::routes())
        .merge(worker_routes)
        .merge(crate::databases::routes())
        .merge(crate::mail::routes())
        .merge(crate::neon_connections::routes())
        .merge(crate::locale::routes())
        .route("/healthz", get(|| async { Json(json!({"status":"ok"})) }))
        .route("/readyz", get(ready))
        .route("/console-config.json", get(console_config))
        .fallback(fallback)
        .layer(axum::extract::DefaultBodyLimit::max(131072))
        .layer(
            CorsLayer::new()
                .allow_origin(origins)
                .allow_credentials(true)
                .allow_methods([
                    Method::GET,
                    Method::POST,
                    Method::PUT,
                    Method::PATCH,
                    Method::DELETE,
                    Method::OPTIONS,
                ])
                .allow_headers([
                    header::CONTENT_TYPE,
                    header::AUTHORIZATION,
                    header::HeaderName::from_static("idempotency-key"),
                    header::HeaderName::from_static("last-event-id"),
                ]),
        )
        .layer(middleware::from_fn_with_state(state.clone(), boundary))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::recovery::quarantine,
        ))
        .with_state(state)
}

async fn fallback(State(state): State<Arc<ServiceState>>, request: Request) -> Response {
    if let Some(ui) = &state.ui
        && crate::ui::ui_path(request.uri().path())
        && crate::browser::request_origin(&state, request.headers()).is_some_and(|origin| {
            [
                state.config.auth_origin.as_str(),
                state.config.console_origin(),
                state.config.studio_origin.as_str(),
            ]
            .contains(&origin)
        })
    {
        return ui.serve(request).await;
    }
    error(StatusCode::NOT_FOUND, "not_found", "Request unavailable.")
}
async fn ready(State(state): State<Arc<ServiceState>>) -> Response {
    // A reachable database is insufficient: verify the authenticated role and exact schema.
    if webdock_persistence::connection::verify_runtime_role(&state.db, "webdock_auth_bootstrap")
        .await
        .is_ok()
        && webdock_persistence::migrations::native_schema_ready(&state.db).await
    {
        return Json(json!({"status":"ready"})).into_response();
    }
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"status":"unavailable"})),
    )
        .into_response()
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicConsoleConfiguration {
    #[serde(skip_serializing_if = "Option::is_none")]
    realtime_url: Option<String>,
    api_origin: String,
    workspace_api_origin: String,
    auth_origin: String,
    enabled_features: Vec<String>,
    database_ordering_enabled: bool,
    locale_preference: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    console_origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    surface: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sso: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    browser_facade: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mail_gateway_origin: Option<String>,
}
fn public_console_configuration(
    config: &PlatformConfig,
    host: Option<&str>,
    sso: bool,
    mail_gateway: Option<&str>,
    locale_preference: &'static str,
) -> PublicConsoleConfiguration {
    let mut result = PublicConsoleConfiguration {
        realtime_url: None,
        api_origin: config.api_origin.clone(),
        workspace_api_origin: config.api_origin.clone(),
        auth_origin: config.auth_origin.clone(),
        enabled_features: config.features.visible(),
        database_ordering_enabled: config.database_new_orders_enabled,
        locale_preference,
        console_origin: None,
        surface: None,
        sso: None,
        browser_facade: None,
        mail_gateway_origin: None,
    };
    let matches = |origin: &str| {
        let url = url::Url::parse(origin).expect("validated origin");
        host == Some(&url[url::Position::BeforeHost..url::Position::AfterPort])
    };
    if matches(&config.auth_origin) {
        result.api_origin.clone_from(&config.auth_origin);
        result.console_origin = Some(config.console_origin().to_owned());
        result.surface = Some("account");
        return result;
    }
    if let Some(origin) = [config.console_origin(), config.studio_origin.as_str()]
        .into_iter()
        .find(|origin| matches(origin))
    {
        result.api_origin = origin.to_owned();
        result.sso = Some(sso);
        result.browser_facade = Some(true);
    }
    if config
        .features
        .require("mail", webdock_core::features::OperationKind::Read)
        .is_ok()
    {
        result.mail_gateway_origin = mail_gateway.map(str::to_owned);
    }
    result
}
async fn console_config(
    State(state): State<Arc<ServiceState>>,
    headers: axum::http::HeaderMap,
) -> Json<PublicConsoleConfiguration> {
    let mut config = public_console_configuration(
        &state.config,
        headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok()),
        state.browser.is_some(),
        state
            .mail
            .as_deref()
            .and_then(crate::mail::MailEndpoints::gateway_origin),
        crate::locale::preference(&headers),
    );
    if config.surface != Some("account") {
        config.realtime_url = state.realtime.as_ref().map(|r| r.url.clone());
    }
    Json(config)
}

#[derive(Clone)]
pub struct ApiFailure {
    pub status: StatusCode,
    pub code: String,
    pub message: String,
}
impl IntoResponse for ApiFailure {
    fn into_response(self) -> Response {
        let mut response = (
            self.status,
            Json(json!({"error":{"code":self.code,"message":self.message}})),
        )
            .into_response();
        response.extensions_mut().insert(self);
        response
    }
}
pub fn reject(status: StatusCode, code: &str, message: &str) -> ApiFailure {
    ApiFailure {
        status,
        code: code.into(),
        message: message.into(),
    }
}
pub fn error(status: StatusCode, code: &str, message: &str) -> Response {
    reject(status, code, message).into_response()
}

fn route_host_allowed(config: &PlatformConfig, ui_enabled: bool, path: &str, origin: &str) -> bool {
    if path == "/console-config.json" {
        return true;
    }
    if path == "/api/locale" || (ui_enabled && crate::ui::ui_path(path)) {
        return [
            config.auth_origin.as_str(),
            config.console_origin(),
            config.studio_origin.as_str(),
        ]
        .contains(&origin);
    }
    if crate::browser::protocol_path(path)
        || path.starts_with("/api/tenant-preview/")
        || path == "/api/portal/studio"
    {
        return origin == config.console_origin() || origin == config.studio_origin;
    }
    if crate::access::protocol_path(path) {
        return origin == config.auth_origin || origin == config.studio_origin;
    }
    if path == "/api/mcp" || crate::resource_auth::protocol_path(path) {
        return origin == config.studio_origin;
    }
    if path.starts_with("/api/console/") {
        return origin == config.console_origin() || origin == config.studio_origin;
    }
    if path.starts_with("/api/auth/")
        || ["/api/databases/gateway", "/api/mail/gateway"].contains(&path)
        || crate::oauth::discovery_path(path)
    {
        return origin == config.auth_origin;
    }
    origin == config.api_origin
}

/// Resolve request authority before dispatch; response hardening still applies to every denial.
fn admit_request(config: &PlatformConfig, ui: bool, request: &Request) -> Result<(), ApiFailure> {
    let headers = request.headers();
    let forwarded = [
        "forwarded",
        "x-forwarded-host",
        "x-forwarded-proto",
        "x-vercel-forwarded-for",
    ]
    .iter()
    .any(|name| headers.contains_key(*name));
    if headers.get_all(header::HOST).iter().count() != 1
        || headers.get_all(header::ORIGIN).iter().count() > 1
        || headers
            .get(header::ORIGIN)
            .is_some_and(|value| value.to_str().is_err())
        || headers.get_all(header::AUTHORIZATION).iter().count() > 1
        || forwarded
    {
        return Err(reject(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Request unavailable.",
        ));
    }
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok());
    let matched = config.allowed_origins().into_iter().find(|origin| {
        url::Url::parse(origin).ok().is_some_and(|url| {
            Some(&url[url::Position::BeforeHost..url::Position::AfterPort]) == host
        })
    });
    let path = request.uri().path();
    if !matched.is_some_and(|origin| route_host_allowed(config, ui, path, origin)) {
        return Err(reject(
            StatusCode::NOT_FOUND,
            "not_found",
            "Request unavailable.",
        ));
    }
    if matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    ) {
        return Ok(());
    }
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok());
    let credential_endpoint = crate::oauth::credential_endpoint(path)
        || crate::resource_auth::protocol_path(path)
        || crate::build::credential_endpoint(path)
        || path == crate::build_webhook::PATH
        || crate::plugins::credential_endpoint(path)
        || crate::plugins::api::credential_endpoint(path)
        || ["/api/databases/gateway", "/api/mail/gateway"].contains(&path);
    if origin.is_none() && credential_endpoint {
        return Ok(());
    }
    if origin.is_some_and(|value| browser_origin_allowed(config, path, matched, value)) {
        return Ok(());
    }
    Err(reject(
        StatusCode::FORBIDDEN,
        "permission_denied",
        "Request origin is not allowed.",
    ))
}

fn browser_origin_allowed(
    config: &PlatformConfig,
    path: &str,
    matched: Option<&str>,
    origin: &str,
) -> bool {
    if path == "/api/locale"
        || crate::browser::protocol_path(path)
        || path.starts_with("/api/tenant-preview/")
        || path == "/api/portal/studio"
        || crate::access::protocol_path(path)
        || path.starts_with("/api/console/")
    {
        return matched == Some(origin);
    }
    if path.starts_with("/api/auth/") {
        return origin == config.auth_origin;
    }
    config.allowed_origins().contains(&origin)
}

async fn boundary(
    State(state): State<Arc<ServiceState>>,
    request: Request,
    next: Next,
) -> Response {
    let request_id = uuid::Uuid::new_v4().to_string();
    let mut response = match admit_request(&state.config, state.ui.is_some(), &request) {
        Ok(()) => next.run(request).await,
        Err(failure) => failure.into_response(),
    };
    if let Some(failure) = response.extensions().get::<ApiFailure>().cloned() {
        *response.body_mut() = axum::body::Body::from(
            json!({"error":{"code":failure.code,"message":failure.message,"requestId":request_id}})
                .to_string(),
        );
        response.headers_mut().remove(header::CONTENT_LENGTH);
    }
    let immutable_ui = response
        .extensions()
        .get::<crate::ui::ImmutableUiAsset>()
        .is_some();
    let headers = response.headers_mut();
    headers.insert(
        "x-request-id",
        request_id
            .parse()
            .expect("generated UUID is a header value"),
    );
    if !immutable_ui {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
}

#[cfg(test)]
mod tests {
    #[test]
    fn public_webmail_configuration_requires_the_feature_and_excludes_account_surface() {
        use std::collections::HashMap;
        let enabled = super::PlatformConfig::from_map(&HashMap::from([(
            "WEBDOCK_FEATURES".to_owned(),
            r#"{"mail":"enabled"}"#.to_owned(),
        )]))
        .unwrap();
        const GATEWAY: &str = "https://mail-gateway.example.test";
        let gateway = Some(GATEWAY);
        let console = super::public_console_configuration(
            &enabled,
            Some("localhost:3120"),
            true,
            gateway,
            "auto",
        );
        let value = serde_json::to_value(console).unwrap();
        assert_eq!(value["mailGatewayOrigin"], GATEWAY);
        assert_eq!(value["apiOrigin"], "http://localhost:3120");
        assert_eq!(value["workspaceApiOrigin"], enabled.api_origin);
        assert_eq!(value["browserFacade"], true);
        assert_eq!(value["sso"], true);
        for (config, host, origin) in [
            (&enabled, "localhost:3125", gateway),
            (&enabled, "localhost:3120", None),
        ] {
            let value = serde_json::to_value(super::public_console_configuration(
                config,
                Some(host),
                false,
                origin,
                "auto",
            ))
            .unwrap();
            assert!(value.get("mailGatewayOrigin").is_none());
        }
        let disabled = super::PlatformConfig::from_map(&HashMap::new()).unwrap();
        let value = serde_json::to_value(super::public_console_configuration(
            &disabled,
            Some("localhost:3120"),
            false,
            gateway,
            "auto",
        ))
        .unwrap();
        assert!(value.get("mailGatewayOrigin").is_none());
    }
    use super::route_host_allowed;
    use std::collections::HashMap;
    use webdock_core::config::PlatformConfig;

    #[test]
    fn route_authorities_preserve_protocol_precedence_and_path_boundaries() {
        let config = PlatformConfig::from_map(&HashMap::from([(
            "WEBDOCK_CONSOLE_ORIGIN".into(),
            "http://localhost:3140".into(),
        )]))
        .unwrap();
        // Origin order: API, Auth, Studio, Console. Keep Console distinct from Studio.
        for (path, allowed) in [
            ("/console-config.json", [true, true, true, true]),
            ("/api/locale", [false, true, true, true]),
            ("/api/locale/", [true, false, false, false]),
            ("/api/sso/login", [false, false, true, true]),
            ("/api/sso", [true, false, false, false]),
            ("/api/users/logout", [false, false, true, true]),
            ("/api/users/logout/", [true, false, false, false]),
            ("/api/users/refresh-token", [false, false, true, true]),
            ("/api/tenant-preview/start", [false, false, true, true]),
            ("/api/tenant-preview/", [false, false, true, true]),
            ("/api/tenant-preview", [false, true, true, false]),
            ("/api/portal/studio", [false, false, true, true]),
            ("/api/portal/studio/", [true, false, false, false]),
            ("/api/access", [false, true, true, false]),
            ("/api/access-extra", [false, true, true, false]),
            ("/api/tenant/fixture", [false, true, true, false]),
            ("/api/invitations", [false, true, true, false]),
            ("/api/sites", [false, true, true, false]),
            ("/api/sites/", [true, false, false, false]),
            ("/api/mcp", [false, false, true, false]),
            ("/api/mcp/", [true, false, false, false]),
            ("/api/console/session", [false, false, true, true]),
            ("/api/console/access", [false, false, true, true]),
            (
                "/api/console/tenant-preview/start",
                [false, false, true, true],
            ),
            ("/api/console", [true, false, false, false]),
            ("/api/auth/sign-in", [false, true, false, false]),
            ("/api/auth/browser/logout", [false, true, false, false]),
            ("/api/auth", [true, false, false, false]),
            ("/api/databases/gateway", [false, true, false, false]),
            ("/api/databases/gateway/", [true, false, false, false]),
            ("/api/mail/gateway", [false, true, false, false]),
            ("/api/mail/gateway/", [true, false, false, false]),
            (
                "/.well-known/openid-configuration",
                [false, true, false, false],
            ),
            (
                "/.well-known/oauth-authorization-server/api/auth",
                [false, true, false, false],
            ),
            (
                "/.well-known/openid-configuration/",
                [true, false, false, false],
            ),
            ("/healthz", [true, false, false, false]),
            ("/readyz", [true, false, false, false]),
            ("/api/unknown", [true, false, false, false]),
        ] {
            for ui_enabled in [false, true] {
                for (origin, expected) in config.allowed_origins().into_iter().zip(allowed) {
                    assert_eq!(
                        route_host_allowed(&config, ui_enabled, path, origin),
                        expected,
                        "{origin} {path} (UI: {ui_enabled})",
                    );
                }
            }
        }
    }

    #[test]
    fn ui_authorities_apply_only_when_ui_is_enabled() {
        let config = PlatformConfig::from_map(&HashMap::from([(
            "WEBDOCK_CONSOLE_ORIGIN".into(),
            "http://localhost:3140".into(),
        )]))
        .unwrap();
        for path in [
            "/",
            "/account",
            "/assets/app.js",
            "/api-like",
            "/console-config.jsonx",
        ] {
            for (ui_enabled, allowed) in [
                (false, [true, false, false, false]),
                (true, [false, true, true, true]),
            ] {
                for (origin, expected) in config.allowed_origins().into_iter().zip(allowed) {
                    assert_eq!(
                        route_host_allowed(&config, ui_enabled, path, origin),
                        expected,
                        "{origin} {path} (UI: {ui_enabled})",
                    );
                }
            }
        }
    }
}
