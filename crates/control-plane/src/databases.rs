//! Browser commands and the confidential canonical Auth gateway endpoint.
use crate::{
    auth,
    http::{ApiFailure, ServiceState, reject},
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use subtle::ConstantTimeEq;
pub struct DatabaseEndpoints {
    pub service: Arc<webdock_databases::DatabaseService>,
    secret_hash: [u8; 32],
}
impl DatabaseEndpoints {
    pub fn new(
        service: Arc<webdock_databases::DatabaseService>,
        secret: &str,
    ) -> Result<Self, &'static str> {
        if !(32..=512).contains(&secret.len()) || secret.bytes().any(|b| b.is_ascii_whitespace()) {
            return Err("Invalid database gateway secret");
        }
        Ok(Self {
            service,
            secret_hash: Sha256::digest(secret.as_bytes()).into(),
        })
    }
    fn gateway_authenticated(&self, headers: &HeaderMap) -> bool {
        let Some(value) = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
        else {
            return false;
        };
        if !(32..=512).contains(&value.len()) || value.bytes().any(|b| b.is_ascii_whitespace()) {
            return false;
        }
        bool::from(self.secret_hash.ct_eq(&Sha256::digest(value.as_bytes())))
    }
}
pub fn routes() -> Router<Arc<ServiceState>> {
    Router::new()
        .route("/api/console/databases", post(command))
        .route("/v1/databases", post(command))
        .route("/api/databases/gateway", post(gateway))
}
fn unavailable() -> ApiFailure {
    reject(
        StatusCode::SERVICE_UNAVAILABLE,
        "unavailable",
        "Database service is unavailable.",
    )
}
fn failure(e: webdock_databases::Error) -> ApiFailure {
    reject(
        StatusCode::from_u16(e.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        "database_error",
        e.message,
    )
}
fn input(headers: &HeaderMap, body: &Bytes, max: usize) -> Result<Value, ApiFailure> {
    if body.len() > max {
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
    serde_json::from_slice(body).map_err(|_| {
        reject(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "Check the request fields.",
        )
    })
}
enum BrowserCommand {
    Registered(webdock_databases::Command),
    Managed(webdock_databases::ManagedCommand),
    Credentials(String),
    Capacity,
}
impl BrowserCommand {
    fn parse(value: Value) -> Result<Self, ApiFailure> {
        if value["action"] == "databases.capacity"
            && value.as_object().is_some_and(|v| v.len() == 1)
        {
            return Ok(Self::Capacity);
        }
        if value["action"] == "databases.credentials" {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Credentials {
                action: String,
                #[serde(rename = "databaseID")]
                database_id: String,
            }
            let command: Credentials = serde_json::from_value(value).map_err(|_| {
                reject(
                    StatusCode::BAD_REQUEST,
                    "invalid_input",
                    "Check the request fields.",
                )
            })?;
            if command.action != "databases.credentials"
                || command
                    .database_id
                    .parse::<i64>()
                    .ok()
                    .is_none_or(|id| id <= 0 || id.to_string() != command.database_id)
            {
                return Err(reject(
                    StatusCode::BAD_REQUEST,
                    "invalid_input",
                    "Check the request fields.",
                ));
            }
            return Ok(Self::Credentials(command.database_id));
        }
        if value["action"]
            .as_str()
            .is_some_and(|s| s.starts_with("databases."))
        {
            return webdock_databases::ManagedCommand::parse(value)
                .map(Self::Managed)
                .map_err(failure);
        }
        webdock_databases::Command::parse(value)
            .map(Self::Registered)
            .map_err(failure)
    }
    fn mutates(&self) -> bool {
        match self {
            Self::Registered(command) => command.mutates(),
            Self::Managed(command) => !command.is_read(),
            Self::Credentials(_) => true,
            Self::Capacity => false,
        }
    }
}
async fn command(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let input = input(&headers, &body, 65536)?;
        let command = BrowserCommand::parse(input.clone())?;
        state
            .config
            .features
            .require(
                "databases",
                if command.mutates() {
                    webdock_core::features::OperationKind::Provision
                } else {
                    webdock_core::features::OperationKind::Read
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
        let service = &state.databases.as_ref().ok_or_else(unavailable)?.service;
        let data = match command {
            BrowserCommand::Capacity => {
                service
                    .capacity_options(&actor.principal, &actor.authority)
                    .await
            }
            BrowserCommand::Registered(_) => {
                service
                    .execute(&actor.principal, &actor.authority, input)
                    .await
            }
            BrowserCommand::Managed(command) => {
                service
                    .execute_managed(&actor.principal, &actor.authority, command.into_value())
                    .await
            }
            BrowserCommand::Credentials(id) => {
                service
                    .display_credentials(&actor.principal, &actor.authority, &id)
                    .await
            }
        }
        .map_err(failure)?;
        Ok(Json(json!({"data":data})).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "lowercase", deny_unknown_fields)]
enum GatewayCommand {
    Exchange { code: String },
    Inspect { token: String },
    Logout { token: String },
}
async fn gateway(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let endpoints = state.databases.as_ref().ok_or_else(unavailable)?;
        if !endpoints.gateway_authenticated(&headers) {
            return Err(reject(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Gateway authentication required.",
            ));
        }
        let input = input(&headers, &body, 2048)?;
        let cmd: GatewayCommand = serde_json::from_value(input).map_err(|_| {
            reject(
                StatusCode::BAD_REQUEST,
                "invalid_input",
                "Check the request fields.",
            )
        })?;
        if !matches!(cmd, GatewayCommand::Logout { .. }) {
            state
                .config
                .features
                .require(
                    "databases",
                    webdock_core::features::OperationKind::ContinueAccepted,
                )
                .map_err(|_| unavailable())?;
        }
        let data = match cmd {
            GatewayCommand::Exchange { code } => endpoints.service.exchange(&code).await,
            GatewayCommand::Inspect { token } => endpoints.service.inspect(&token).await,
            GatewayCommand::Logout { token } => endpoints.service.logout(&token).await,
        }
        .map_err(failure)?;
        Ok(Json(json!({"data":data})).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser_commands_keep_management_and_credentials_strict() {
        assert!(
            !BrowserCommand::parse(json!({"action":"databases.capacity"}))
                .ok()
                .unwrap()
                .mutates()
        );
        assert!(
            BrowserCommand::parse(json!({"action":"databases.capacity","subject":"forged"}))
                .is_err()
        );
        assert!(
            !BrowserCommand::parse(json!({"action":"databases.get","databaseID":"1"}))
                .ok()
                .unwrap()
                .mutates()
        );
        assert!(BrowserCommand::parse(json!({"action":"databases.pause","databaseID":"1","revision":1,"idempotencyKey":"fixture-pause-1234"})).ok().unwrap().mutates());
        assert!(
            BrowserCommand::parse(json!({"action":"databases.credentials","databaseID":"1"}))
                .ok()
                .unwrap()
                .mutates()
        );
        assert!(
            BrowserCommand::parse(
                json!({"action":"databases.credentials","databaseID":"1","subject":"forged"})
            )
            .is_err()
        );
        assert!(
            BrowserCommand::parse(
                json!({"action":"databases.credentials","databaseID":"9223372036854775808"})
            )
            .is_err()
        );
        assert!(
            !BrowserCommand::parse(json!({"action":"list","customerID":"1"}))
                .ok()
                .unwrap()
                .mutates()
        );
    }
}
