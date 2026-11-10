use crate::{
    auth,
    http::{ApiFailure, ServiceState, reject},
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::Value;
use spitzli_core::features::OperationKind;
use std::sync::Arc;

pub fn routes() -> Router<Arc<ServiceState>> {
    Router::new()
        .merge(crate::build_logs::routes())
        .merge(crate::build_infrastructure::routes())
        .route("/api/console/hosting", post(command))
        .route("/v1/hosting", post(command))
        .route("/api/console/hosting/reads/{id}", get(read))
        .route("/v1/hosting/reads/{id}", get(read))
}
fn unavailable() -> ApiFailure {
    reject(
        StatusCode::SERVICE_UNAVAILABLE,
        "unavailable",
        "Hosting is unavailable.",
    )
}
fn failure(error: spitzli_hosting::HostingError) -> ApiFailure {
    reject(
        StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        "hosting_error",
        &error.message,
    )
}
fn unauthorized() -> ApiFailure {
    reject(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "Sign in to continue.",
    )
}
async fn read(
    State(state): State<Arc<ServiceState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let result = async {
        state
            .config
            .features
            .require("hosting", OperationKind::Read)
            .map_err(|_| unavailable())?;
        let actor = auth::resolve_actor(&state, &headers)
            .await?
            .ok_or_else(unauthorized)?;
        let service = state.hosting.as_ref().ok_or_else(unavailable)?;
        let view = service
            .get_read(&actor.principal, &id)
            .await
            .map_err(failure)?;
        // This read may wait for database locks. Recheck the browser grant and
        // current authority immediately before releasing its protected result.
        let current = auth::resolve_actor(&state, &headers)
            .await?
            .ok_or_else(unauthorized)?;
        if current.principal.user_id != actor.principal.user_id
            || current.principal.session_id != actor.principal.session_id
            || current.principal.role() != actor.principal.role()
            || current.principal.in_tenant_preview() != actor.principal.in_tenant_preview()
            || current.read_only() != actor.read_only()
        {
            return Err(unauthorized());
        }
        Ok(Json(view).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
async fn command(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result = async {
        if body.len() > 65536 {
            return Err(reject(
                StatusCode::PAYLOAD_TOO_LARGE,
                "invalid_input",
                "Request is too large.",
            ));
        }
        if headers.get_all(header::CONTENT_TYPE).iter().count() != 1
            || headers
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(';').next())
                .map(str::trim)
                != Some("application/json")
        {
            return Err(reject(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "invalid_input",
                "JSON is required.",
            ));
        }
        let input: Value = serde_json::from_slice(&body).map_err(|_| {
            reject(
                StatusCode::BAD_REQUEST,
                "invalid_input",
                "Check the request fields.",
            )
        })?;
        let action = input.get("action").and_then(Value::as_str).ok_or_else(|| {
            reject(
                StatusCode::BAD_REQUEST,
                "invalid_input",
                "Action is required.",
            )
        })?;
        // The domain repeats admission against persisted module state and current permissions.
        let read = action.ends_with(".get")
            || action.ends_with(".list")
            || [
                "apps.logs",
                "apps.inspect",
                "apps.metrics",
                "apps.deletion",
                "apps.storageDeletion",
                "operations.overview",
                "usage.get",
            ]
            .contains(&action);
        state
            .config
            .features
            .require(
                "hosting",
                if read {
                    OperationKind::Read
                } else {
                    OperationKind::Provision
                },
            )
            .map_err(|_| unavailable())?;
        let actor = auth::resolve_actor(&state, &headers)
            .await?
            .ok_or_else(|| {
                reject(
                    StatusCode::UNAUTHORIZED,
                    "unauthorized",
                    "Sign in to continue.",
                )
            })?;
        if action.starts_with("git.") {
            return crate::build::execute(&state, &actor, input).await;
        }
        let service = state.hosting.as_ref().ok_or_else(unavailable)?;
        if action.starts_with("recovery.") {
            if actor.read_only() {
                return Err(reject(
                    StatusCode::FORBIDDEN,
                    "permission_denied",
                    "Recovery requires platform operator authority.",
                ));
            }
            let authority = serde_json::from_value(
                serde_json::to_value(&actor.authority).map_err(|_| unavailable())?,
            )
            .map_err(|_| unavailable())?;
            let value = service
                .recovery_command(&actor.principal, &authority, input)
                .await
                .map_err(failure)?;
            return Ok(Json(value).into_response());
        }
        let value = service
            .execute_with_ceiling(
                &actor.principal,
                input,
                spitzli_hosting::HostingCeiling {
                    read: true,
                    write: !actor.read_only(),
                },
            )
            .await
            .map_err(failure)?;
        Ok(Json(value).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
