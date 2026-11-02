//! Restored Core must remain closed before authentication, refresh or worker routes.
//! Wired only after the coordinated migration/compiler hold is released.
use crate::http::ServiceState;
use axum::{
    Json,
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use sea_orm::{ConnectionTrait, DatabaseConnectionType, DbBackend, Statement};
use serde_json::json;
use std::sync::Arc;
pub(crate) async fn quarantine(
    State(state): State<Arc<ServiceState>>,
    request: Request,
    next: Next,
) -> Response {
    // The explicit no-database router has no usable domain authority. Real pools
    // always query the durable epoch; a connection failure cannot open a cold target.
    if matches!(&state.db.inner, DatabaseConnectionType::Disconnected) {
        return next.run(request).await;
    }
    let state_value = state
        .db
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT webdock_rls.recovery_target_quarantined() AS closed",
        ))
        .await;
    match state_value {
        Ok(Some(row)) if row.try_get::<bool>("","closed")==Ok(false)=>next.run(request).await,
        Ok(Some(row)) if row.try_get::<bool>("","closed")==Ok(true)=>{
            (StatusCode::LOCKED,Json(json!({"error":{"code":"recovery_quarantined","message":"This restored installation is isolated. Operator reconciliation is required."}}))).into_response()
        },
        _=>(StatusCode::SERVICE_UNAVAILABLE,Json(json!({"error":{"code":"recovery_authority_unavailable","message":"Installation authority is unavailable."}}))).into_response(),
    }
}
