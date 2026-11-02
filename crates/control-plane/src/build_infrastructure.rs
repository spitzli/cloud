use crate::{
    auth,
    http::{ApiFailure, ServiceState, reject},
};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::Value;
use std::sync::Arc;
pub(crate) fn routes() -> Router<Arc<ServiceState>> {
    Router::new().route("/api/console/hosting/build-infrastructure", post(command))
}
async fn command(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Response {
    let result = async {
        state
            .config
            .features
            .require("hosting", webdock_core::features::OperationKind::Read)
            .map_err(|_| {
                reject(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Build infrastructure is unavailable.",
                )
            })?;
        let actor = auth::resolve_actor(&state, &headers)
            .await?
            .ok_or_else(|| {
                reject(
                    StatusCode::UNAUTHORIZED,
                    "authentication_required",
                    "Authentication required.",
                )
            })?;
        if !actor.principal.is_operator() || actor.read_only() {
            return Err(reject(
                StatusCode::FORBIDDEN,
                "operator_required",
                "Operator authority required.",
            ));
        }
        let service = state.build.as_ref().ok_or_else(|| {
            reject(
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                "Build infrastructure is unavailable.",
            )
        })?;
        let value = service
            .execute_build_infrastructure(&actor.principal, &actor.authority, input)
            .await
            .map_err(|e| {
                reject(
                    StatusCode::from_u16(e.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                    e.code,
                    "Build infrastructure request could not be completed.",
                )
            })?;
        let mut response = Json(value).into_response();
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("no-store"),
        );
        Ok::<_, ApiFailure>(response)
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
