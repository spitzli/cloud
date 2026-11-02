//! Native account routes and the same-origin Console facade share one identity service.
use crate::http::{ApiFailure, ServiceState, reject as error};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use webdock_identity::{
    IdentityError, IdentityService, IssuedSession, MfaProof, PasswordAuthentication, Principal,
};

pub fn routes() -> Router<Arc<ServiceState>> {
    Router::new()
        .route("/v1/auth/session", get(session))
        .route("/api/console/session", get(session))
        .route("/api/auth/get-session", get(native_session))
        .route("/api/console/auth/get-session", get(native_session))
        .route("/api/auth/list-sessions", get(sessions))
        .route("/api/console/auth/list-sessions", get(sessions))
        .route(
            "/api/auth/passkey/{operation}",
            get(passkey_get).post(passkey_action),
        )
        .route("/api/auth/{*action}", post(action))
        .route("/api/console/auth/{*action}", post(action))
}

fn service(state: &ServiceState) -> Result<&IdentityService, ApiFailure> {
    state.identity.as_deref().ok_or_else(|| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Identity service is not configured.",
        )
    })
}
pub fn cookie(headers: &HeaderMap) -> Result<String, ApiFailure> {
    let parts: Result<Vec<_>, _> = headers
        .get_all(header::COOKIE)
        .iter()
        .map(|header| header.to_str())
        .collect();
    parts.map(|parts| parts.join("; ")).map_err(|_| {
        error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Invalid request.",
        )
    })
}
pub fn public_user(actor: &Principal) -> Value {
    json!({"id":actor.user_id,"name":actor.name,"email":actor.email,"emailVerified":actor.email_verified(),"role":if actor.in_tenant_preview() { "user" } else { actor.role() },"banned":false,"twoFactorEnabled":actor.two_factor_enabled(),"mustChangePassword":actor.requires_password_change(),"preview":actor.in_tenant_preview(),"acting":actor.is_acting_customer(),"customerScope":actor.customer_scope()})
}
fn session_value(actor: &Principal) -> Value {
    json!({"user":public_user(actor),"session":{"id":actor.session_id,"userId":actor.user_id,"expiresAt":actor.expires_at}})
}
pub fn failure(failure: IdentityError) -> ApiFailure {
    match failure {
        IdentityError::InvalidInput => error(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "Check the request fields.",
        ),
        IdentityError::SecurityState => error(
            StatusCode::FORBIDDEN,
            "security_setup_required",
            "Complete account security setup.",
        ),
        IdentityError::RateLimited => error(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Try again later.",
        ),
        IdentityError::Database(_) | IdentityError::Configuration | IdentityError::Worker => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Identity service is unavailable.",
        ),
        _ => error(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "Authentication could not be verified.",
        ),
    }
}
/// Server-owned proof reference. Domain jobs must revalidate it rather than trust cached roles.
pub use webdock_browser_session::StoredAuthority as ActorAuthority;
pub struct AuthenticatedActor {
    pub principal: Principal,
    pub authority: ActorAuthority,
}
impl AuthenticatedActor {
    pub fn read_only(&self) -> bool {
        self.principal.in_read_only_preview()
            || matches!(&self.authority,ActorAuthority::FirstPartyBrowser{browser} if browser.read_only)
    }
}
pub async fn resolve_actor(
    state: &ServiceState,
    headers: &HeaderMap,
) -> Result<Option<AuthenticatedActor>, ApiFailure> {
    let raw = cookie(headers)?;
    if let Some(browser) = &state.browser
        && let Some(origin) = crate::browser::request_origin(state, headers)
        && [
            state.config.console_origin(),
            state.config.studio_origin.as_str(),
        ]
        .contains(&origin)
    {
        return browser
            .resolve_authority(origin, &raw)
            .await
            .map(|value| {
                value.map(|v| AuthenticatedActor {
                    principal: v.principal,
                    authority: ActorAuthority::FirstPartyBrowser {
                        browser: v.authority,
                    },
                })
            })
            .map_err(crate::browser::failure);
    }
    service(state)?
        .authenticate_cookie(&raw)
        .await
        .map(|value| {
            value.map(|principal| AuthenticatedActor {
                authority: ActorAuthority::local_session(&principal),
                principal,
            })
        })
        .map_err(failure)
}
pub async fn resolve(
    state: &ServiceState,
    headers: &HeaderMap,
) -> Result<Option<Principal>, ApiFailure> {
    resolve_actor(state, headers)
        .await
        .map(|value| value.map(|v| v.principal))
}

async fn session(State(state): State<Arc<ServiceState>>, headers: HeaderMap) -> Response {
    match resolve(&state, &headers).await {
        Ok(actor) => Json(json!({"data":actor.as_ref().map(session_value)})).into_response(),
        Err(error) => error.into_response(),
    }
}
async fn native_session(State(state): State<Arc<ServiceState>>, headers: HeaderMap) -> Response {
    match resolve(&state, &headers).await {
        Ok(actor) => Json(actor.as_ref().map(session_value)).into_response(),
        Err(error) => error.into_response(),
    }
}
async fn sessions(State(state): State<Arc<ServiceState>>, headers: HeaderMap) -> Response {
    let result = async {
        service(&state)?
            .list_sessions(&cookie(&headers)?)
            .await
            .map_err(failure)
    }
    .await;
    match result {
        Ok(sessions) => Json(sessions).into_response(),
        Err(error) => error.into_response(),
    }
}
fn issued(session: IssuedSession, clear_mfa: Option<String>) -> Response {
    let mut response =
        Json(json!({"user":public_user(&session.principal),"redirect":false})).into_response();
    match session.set_cookie.parse() {
        Ok(value) => {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "Request failed.",
            )
            .into_response();
        }
    }
    if let Some(cookie) = clear_mfa.and_then(|value| value.parse().ok()) {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    response
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SignIn {
    email: String,
    password: String,
    #[serde(default)]
    remember_me: Option<bool>,
    #[serde(default)]
    callback_url: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PasswordChange {
    current_password: String,
    new_password: String,
    #[serde(default)]
    revoke_other_sessions: Option<bool>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Password {
    password: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Code {
    code: String,
    #[serde(default)]
    trust_device: Option<bool>,
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, ApiFailure> {
    serde_json::from_slice(body).map_err(|_| {
        error(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "Check the request fields.",
        )
    })
}

fn passkey_origin(state: &ServiceState, headers: &HeaderMap) -> Result<(), ApiFailure> {
    // Same-origin GET fetches may omit Origin. Fetch Metadata is browser-owned;
    // navigations and cross-origin fetches do not satisfy this alternative.
    if !headers.contains_key(header::ORIGIN)
        && headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) == Some("same-origin")
        && matches!(
            headers.get("sec-fetch-mode").and_then(|v| v.to_str().ok()),
            Some("cors" | "same-origin")
        )
    {
        return Ok(());
    }
    if headers.get_all(header::ORIGIN).iter().count() != 1
        || headers.get(header::ORIGIN).and_then(|v| v.to_str().ok())
            != Some(state.config.auth_origin.as_str())
    {
        return Err(error(
            StatusCode::FORBIDDEN,
            "permission_denied",
            "Use the account origin for passkeys.",
        ));
    }
    Ok(())
}
fn with_cookie(value: Value, cookie: String) -> Result<Response, ApiFailure> {
    let mut response = Json(value).into_response();
    response.headers_mut().append(
        header::SET_COOKIE,
        cookie.parse().map_err(|_| {
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "Request failed.",
            )
        })?,
    );
    Ok(response)
}
async fn passkey_get(
    State(state): State<Arc<ServiceState>>,
    Path(operation): Path<String>,
    headers: HeaderMap,
) -> Response {
    let result = async {
        passkey_origin(&state, &headers)?;
        let identity = service(&state)?;
        if state.auth_limiter.check().is_err() {
            return Err(failure(IdentityError::RateLimited));
        }
        match operation.as_str() {
            "generate-register-options" => {
                let challenge = identity
                    .begin_passkey_registration(&cookie(&headers)?)
                    .await
                    .map_err(failure)?;
                with_cookie(json!(challenge.options.public_key), challenge.set_cookie)
            }
            "generate-authenticate-options" => {
                let challenge = identity
                    .begin_passkey_authentication()
                    .await
                    .map_err(failure)?;
                with_cookie(json!(challenge.options.public_key), challenge.set_cookie)
            }
            "list-user-passkeys" => Ok(Json(
                identity
                    .list_passkeys(&cookie(&headers)?)
                    .await
                    .map_err(failure)?,
            )
            .into_response()),
            _ => Err(error(
                StatusCode::NOT_FOUND,
                "not_found",
                "Request unavailable.",
            )),
        }
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

async fn passkey_action(
    State(state): State<Arc<ServiceState>>,
    Path(operation): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    perform(&state, &format!("passkey/{operation}"), &headers, &body)
        .await
        .unwrap_or_else(IntoResponse::into_response)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResetRequest {
    email: String,
    #[serde(default)]
    redirect_to: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResetPassword {
    token: String,
    new_password: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetSession {
    token: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PasskeyRegistration {
    response: webdock_identity::RegisterPublicKeyCredential,
    name: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PasskeyAuthentication {
    response: webdock_identity::PublicKeyCredential,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PasskeyTarget {
    id: String,
    #[serde(default)]
    name: Option<String>,
}

async fn action(
    State(state): State<Arc<ServiceState>>,
    Path(action): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = perform(&state, &action, &headers, &body).await;
    match result {
        Ok(response) => response,
        Err(response) => response.into_response(),
    }
}
async fn perform(
    state: &ServiceState,
    action: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Response, ApiFailure> {
    if body.len() > 16384 {
        return Err(error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "invalid_input",
            "Request is too large.",
        ));
    }
    if !headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(';').next().unwrap_or("").trim() == "application/json")
    {
        return Err(error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "invalid_input",
            "JSON is required.",
        ));
    }
    if state.auth_limiter.check().is_err() {
        return Err(error(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Try again later.",
        ));
    }
    if action.starts_with("passkey/") {
        passkey_origin(state, headers)?;
    }
    let identity = service(state)?;
    let cookie = cookie(headers)?;
    match action {
        "sign-in/email" => {
            let input: SignIn = parse(body)?;
            // These client hints must never extend server session lifetime or redirect off origin.
            let _ = (input.remember_me, input.callback_url);
            match identity
                .authenticate_password(&input.email, &input.password)
                .await
                .map_err(failure)?
            {
                PasswordAuthentication::Invalid => Err(error(
                    StatusCode::UNAUTHORIZED,
                    "invalid_credentials",
                    "Authentication could not be verified.",
                )),
                PasswordAuthentication::SetupRequired {
                    session: Some(session),
                    ..
                } => Ok(issued(session, None)),
                PasswordAuthentication::SetupRequired { session: None, .. } => Err(error(
                    StatusCode::FORBIDDEN,
                    "setup_required",
                    "Complete account activation.",
                )),
                PasswordAuthentication::Authenticated(session) => Ok(issued(session, None)),
                PasswordAuthentication::MfaRequired { set_cookie, .. } => {
                    let mut response = Json(json!({"twoFactorRedirect":true})).into_response();
                    response.headers_mut().append(
                        header::SET_COOKIE,
                        set_cookie.parse().map_err(|_| {
                            error(
                                StatusCode::INTERNAL_SERVER_ERROR,
                                "internal_error",
                                "Request failed.",
                            )
                        })?,
                    );
                    Ok(response)
                }
            }
        }
        "sign-out" => {
            parse::<serde_json::Map<String, Value>>(body)?;
            identity.logout(&cookie).await.map_err(failure)?;
            let mut response = Json(json!({"success":true})).into_response();
            for value in [identity.clear_cookie(), identity.clear_mfa_cookie()] {
                response.headers_mut().append(
                    header::SET_COOKIE,
                    value.parse().map_err(|_| {
                        error(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "internal_error",
                            "Request failed.",
                        )
                    })?,
                );
            }
            Ok(response)
        }
        "link-neon" => {
            let input: Password = parse(body)?;
            identity
                .link_neon_identity(&cookie, &input.password)
                .await
                .map_err(failure)?;
            Ok(Json(json!({"status":true})).into_response())
        }
        "change-password" => {
            let input: PasswordChange = parse(body)?;
            let _ = input.revoke_other_sessions;
            if identity
                .change_managed_password(&cookie, &input.current_password, &input.new_password)
                .await
                .map_err(failure)?
            {
                return with_cookie(
                    json!({"status":true,"requiresSignIn":true}),
                    identity.clear_cookie(),
                );
            }
            Ok(issued(
                identity
                    .change_password(&cookie, &input.current_password, &input.new_password)
                    .await
                    .map_err(failure)?,
                None,
            ))
        }
        "two-factor/enable" => {
            let input: Password = parse(body)?;
            let enrollment = identity
                .begin_totp_enrollment(&cookie, &input.password)
                .await
                .map_err(failure)?;
            Ok(
                Json(json!({"totpURI":enrollment.totp_uri,"backupCodes":enrollment.backup_codes}))
                    .into_response(),
            )
        }
        "two-factor/verify-totp" | "two-factor/verify-backup-code" => {
            let input: Code = parse(body)?;
            let _ = input.trust_device;
            let actor = identity
                .authenticate_cookie(&cookie)
                .await
                .map_err(failure)?;
            let session = if actor.is_some_and(|actor| !actor.two_factor_enabled())
                && action == "two-factor/verify-totp"
            {
                identity.confirm_totp_enrollment(&cookie, &input.code).await
            } else {
                let proof = if action.ends_with("backup-code") {
                    MfaProof::RecoveryCode(input.code)
                } else {
                    MfaProof::Totp(input.code)
                };
                identity.complete_mfa(&cookie, proof).await
            }
            .map_err(failure)?;
            Ok(issued(session, Some(identity.clear_mfa_cookie())))
        }
        "revoke-other-sessions" => {
            parse::<serde_json::Map<String, Value>>(body)?;
            identity
                .revoke_other_sessions(&cookie)
                .await
                .map_err(failure)?;
            Ok(Json(json!({"status":true})).into_response())
        }
        "revoke-session-by-id" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Target {
                id: String,
            }
            let input: Target = parse(body)?;
            identity
                .revoke_owned_session(&cookie, &input.id)
                .await
                .map_err(failure)?;
            Ok(Json(json!({"status":true})).into_response())
        }
        "revoke-session" => {
            let input: TargetSession = parse(body)?;
            identity
                .revoke_session_by_token(&cookie, &input.token)
                .await
                .map_err(failure)?;
            Ok(Json(json!({"status":true})).into_response())
        }
        "two-factor/disable" => {
            let input: Password = parse(body)?;
            Ok(issued(
                identity
                    .disable_two_factor(&cookie, &input.password)
                    .await
                    .map_err(failure)?,
                Some(identity.clear_mfa_cookie()),
            ))
        }
        "two-factor/generate-backup-codes" => {
            let input: Password = parse(body)?;
            Ok(Json(json!({"backupCodes":identity.regenerate_recovery_codes(&cookie,&input.password).await.map_err(failure)?})).into_response())
        }
        "request-password-reset" => {
            let input: ResetRequest = parse(body)?;
            // Recovery destinations are server-owned; never deliver attacker-supplied links.
            let _ = input.redirect_to;
            identity
                .request_password_reset(&input.email)
                .await
                .map_err(failure)?;
            Ok(Json(json!({"status":true,"message":"If the account is eligible, recovery instructions will be sent."})).into_response())
        }
        "reset-password" => {
            let input: ResetPassword = parse(body)?;
            match identity
                .complete_password_reset(&input.token, &input.new_password)
                .await
            {
                Ok(email) => Ok(Json(json!({"status":true,"email":email})).into_response()),
                Err(IdentityError::Unauthorized) => {
                    if identity
                        .reset_managed_password(&input.token, &input.new_password)
                        .await
                        .map_err(failure)?
                    {
                        Ok(Json(json!({"status":true})).into_response())
                    } else {
                        Err(failure(IdentityError::Unauthorized))
                    }
                }
                Err(error) => Err(failure(error)),
            }
        }
        "passkey/verify-registration" => {
            let input: PasskeyRegistration = parse(body)?;
            let passkey = identity
                .finish_passkey_registration(&cookie, &input.response, input.name.as_deref())
                .await
                .map_err(failure)?;
            with_cookie(json!(passkey), identity.clear_passkey_cookie())
        }
        "passkey/verify-authentication" => {
            let input: PasskeyAuthentication = parse(body)?;
            Ok(issued(
                identity
                    .finish_passkey_authentication(&cookie, &input.response)
                    .await
                    .map_err(failure)?,
                Some(identity.clear_passkey_cookie()),
            ))
        }
        "passkey/delete-passkey" | "passkey/update-passkey" => {
            let input: PasskeyTarget = parse(body)?;
            let changed = if action.ends_with("delete-passkey") {
                identity.delete_passkey(&cookie, &input.id).await
            } else {
                identity
                    .rename_passkey(
                        &cookie,
                        &input.id,
                        input
                            .name
                            .as_deref()
                            .ok_or_else(|| failure(IdentityError::InvalidInput))?,
                    )
                    .await
            }
            .map_err(failure)?;
            if !changed {
                return Err(error(
                    StatusCode::NOT_FOUND,
                    "not_found",
                    "Passkey unavailable.",
                ));
            }
            Ok(Json(json!({"status":true})).into_response())
        }
        _ => Err(error(
            StatusCode::NOT_FOUND,
            "not_found",
            "Request unavailable.",
        )),
    }
}
