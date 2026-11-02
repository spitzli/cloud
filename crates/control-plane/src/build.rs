//! Native Build browser dispatch and the separate authenticated worker boundary.
use crate::{
    auth::AuthenticatedActor,
    http::{ApiFailure, ServiceState, reject},
};
use axum::{
    Json, Router,
    extract::{Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use serde_json::Value;
use std::sync::Arc;
use webdock_core::features::OperationKind;
fn unavailable() -> ApiFailure {
    reject(
        StatusCode::SERVICE_UNAVAILABLE,
        "unavailable",
        "Git builds are unavailable.",
    )
}
pub(crate) async fn execute(
    state: &ServiceState,
    actor: &AuthenticatedActor,
    input: Value,
) -> Result<Response, ApiFailure> {
    let action = input["action"].as_str().ok_or_else(|| {
        reject(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "Action is required.",
        )
    })?;
    state
        .config
        .features
        .require(
            "hosting",
            if webdock_build::contracts::read_action(action) {
                OperationKind::Read
            } else {
                OperationKind::Provision
            },
        )
        .map_err(|_| unavailable())?;
    let service = state.build.as_ref().ok_or_else(unavailable)?;
    let value = service
        .execute_with_authority(
            &actor.principal,
            &actor.authority,
            input,
            webdock_build::BuildCeiling {
                read: true,
                write: !actor.read_only(),
            },
        )
        .await
        .map_err(|e| {
            reject(
                StatusCode::from_u16(e.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                e.code,
                "Git build request could not be completed.",
            )
        })?;
    Ok(Json(value).into_response())
}
/// Only these retained worker POST endpoints may omit browser Origin. Each still requires
/// current persisted worker credentials inside the domain router; browser commands never use it.
pub(crate) fn credential_endpoint(path: &str) -> bool {
    path.strip_prefix("/api/hosting/git/worker/")
        .is_some_and(|tail| {
            matches!(
                tail,
                "claim"
                    | "kubernetes-log"
                    | "kubernetes-claim"
                    | "kubernetes-attempts"
                    | "kubernetes-intent"
                    | "kubernetes-job"
                    | "kubernetes-retire"
                    | "complete"
                    | "release-claim"
                    | "release-prepare"
                    | "container-publish"
                    | "observe"
                    | "release-complete"
            )
        })
}
pub(crate) fn worker_routes(
    service: Option<Arc<webdock_build::BuildService>>,
    features: webdock_core::features::FeatureSet,
) -> Router<Arc<ServiceState>> {
    let Some(service) = service else {
        return Router::new();
    };
    Router::new()
        .nest_service(
            "/api/hosting/git/worker",
            webdock_build::http::worker_router(service),
        )
        .route_layer(middleware::from_fn_with_state(features, worker_boundary))
}

async fn worker_boundary(
    State(features): State<webdock_core::features::FeatureSet>,
    request: Request,
    next: Next,
) -> Response {
    if features
        .require("hosting", OperationKind::ContinueAccepted)
        .is_err()
    {
        return unavailable().into_response();
    }
    next.run(request).await
}
