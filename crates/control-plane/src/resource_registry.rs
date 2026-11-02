use crate::resource_auth::ResourceFailure;
use crate::{
    http::ServiceState,
    resource_auth::{self, Resource},
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{OriginalUri, State},
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use webdock_oauth::McpAuthentication;

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Collection {
    Customers,
    Projects,
    AuditEvents,
}
impl Collection {
    pub fn name(self) -> &'static str {
        match self {
            Self::Customers => "customers",
            Self::Projects => "projects",
            Self::AuditEvents => "audit-events",
        }
    }
    fn parse(value: &str) -> Result<Self, ResourceFailure> {
        serde_json::from_value(json!(value))
            .map_err(|_| public_error(StatusCode::NOT_FOUND, "Registry collection unavailable."))
    }
}
pub(crate) enum Operation {
    List(Value),
    Get {
        collection: Collection,
        id: String,
    },
    Save {
        collection: Collection,
        id: Option<String>,
        data: Value,
    },
    Archive {
        collection: Collection,
        id: String,
        archived: bool,
    },
}
impl Operation {
    pub fn write(&self) -> bool {
        matches!(self, Self::Save { .. } | Self::Archive { .. })
    }
}
pub(crate) fn public_error(status: StatusCode, message: &str) -> ResourceFailure {
    ResourceFailure::new((status, Json(json!({"error":message}))).into_response())
}
pub(crate) fn unavailable() -> ResourceFailure {
    public_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "Resource service is unavailable.",
    )
}
pub(crate) fn read_json(headers: &HeaderMap, bytes: &[u8]) -> Result<Value, ResourceFailure> {
    if bytes.len() > 65536 {
        return Err(public_error(
            StatusCode::PAYLOAD_TOO_LARGE,
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
        return Err(public_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Use application/json.",
        ));
    }
    serde_json::from_slice(bytes)
        .map_err(|_| public_error(StatusCode::BAD_REQUEST, "Invalid JSON."))
}
pub(crate) fn schemas() -> Value {
    serde_json::from_str(include_str!("resource-registry.schema.json"))
        .expect("bundled registry schemas")
}
pub fn routes() -> Router<Arc<ServiceState>> {
    Router::new()
        .route("/api/registry", any(handle))
        .route("/api/registry/{*path}", any(handle))
}
pub(crate) async fn execute(
    state: &ServiceState,
    actor: &McpAuthentication,
    operation: Operation,
) -> Result<Value, ResourceFailure> {
    if operation.write() && !actor.authority.can_write() {
        return Err(resource_auth::challenge(
            state,
            "webdock:read webdock:write",
            StatusCode::FORBIDDEN,
        ));
    }
    let registry = state.registry.as_ref().ok_or_else(unavailable)?;
    let oauth = state.oauth.as_ref().ok_or_else(unavailable)?;
    let service = registry
        .with_mcp_authority(oauth, &actor.authority)
        .await
        .map_err(domain_error)?;
    let principal = &actor.principal;
    match operation {
        Operation::List(input) => service.list(principal, input).await,
        Operation::Get { collection, id } => service.get(principal, collection.name(), &id).await,
        Operation::Save {
            collection: Collection::Customers,
            id,
            data,
        } => service.save_customer(principal, id.as_deref(), data).await,
        Operation::Save {
            collection: Collection::Projects,
            id,
            data,
        } => service.save_project(principal, id.as_deref(), data).await,
        Operation::Save {
            collection: Collection::AuditEvents,
            ..
        } => return Err(method_not_allowed("GET")),
        Operation::Archive {
            collection,
            id,
            archived,
        } => {
            service
                .archive(principal, collection.name(), &id, archived)
                .await
        }
    }
    .map_err(domain_error)
}
fn domain_error(error: webdock_registry::RegistryError) -> ResourceFailure {
    let status = StatusCode::from_u16(error.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    public_error(
        status,
        &if status.is_server_error() {
            "The operation failed. No change was confirmed.".to_owned()
        } else {
            error.to_string()
        },
    )
}
fn method_not_allowed(allow: &'static str) -> ResourceFailure {
    ResourceFailure::new((StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, allow)]).into_response())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchiveInput {
    archived: bool,
}
async fn handle(
    State(state): State<Arc<ServiceState>>,
    OriginalUri(uri): OriginalUri,
    method: Method,
    headers: HeaderMap,
    bytes: Bytes,
) -> Response {
    let result=async {
        let actor=resource_auth::authenticate(&state,&headers,Resource::Registry,method!=Method::GET).await?;
        let path=uri.path().strip_prefix("/api/registry").unwrap_or("").trim_start_matches('/');
        if path.is_empty() {
            if method!=Method::GET { return Err(method_not_allowed("GET")); }
            // Discovery itself is an authorized registry read, including live mapped-operator access.
            execute(&state,&actor,Operation::List(json!({"collection":"customers","limit":1}))).await?;
            return Ok(Json(json!({"resource":format!("{}/api/mcp",state.config.studio_origin),"scopes":["webdock:read","webdock:write"],"collections":["customers","projects","audit-events"],"operations":{"GET":"/api/registry/{collection}[/{id}]","POST":"/api/registry/{collection}","PUT":"/api/registry/{collection}/{id}","PATCH":"customers or projects /{id}: { archived: boolean }"},"schemas":schemas(),"notes":"Bearer authorization only. Uses the same resource and scopes as MCP. Creates are not idempotent; do not retry ambiguous writes automatically."})).into_response());
        }
        let segments=path.split('/').collect::<Vec<_>>();
        let [collection,tail @ ..]=segments.as_slice() else { return Err(public_error(StatusCode::NOT_FOUND,"Not found")); };
        if tail.len()>1 { return Err(public_error(StatusCode::NOT_FOUND,"Not found")); }
        let collection=Collection::parse(collection)?;
        let id=tail.first().map(|v|(*v).to_owned());
        if let Some(id)=&id { webdock_registry::contracts::record_id(id).map_err(domain_error)?; }
        let operation=match (method,collection,id) {
            (Method::GET,collection,Some(id))=>Operation::Get{collection,id},
            (Method::GET,collection,None)=>Operation::List(crate::registry::list_query(collection.name(),uri.query()).map_err(ResourceFailure::from)?),
            (_,Collection::AuditEvents,_)=>return Err(method_not_allowed("GET")),
            (Method::POST,collection,None)=>Operation::Save{collection,id:None,data:read_json(&headers,&bytes)?},
            (Method::PUT,collection,Some(id))=>Operation::Save{collection,id:Some(id),data:read_json(&headers,&bytes)?},
            (Method::PATCH,collection,Some(id))=>{
                let input:ArchiveInput=serde_json::from_value(read_json(&headers,&bytes)?).map_err(|_|public_error(StatusCode::BAD_REQUEST,"Expected archived boolean."))?;
                Operation::Archive{collection,id,archived:input.archived}
            }
            (_,_,Some(_))=>return Err(method_not_allowed("GET, PUT, PATCH")),
            _=>return Err(method_not_allowed("GET, POST")),
        };
        let status=if matches!(&operation,Operation::Save{id:None,..}){StatusCode::CREATED}else{StatusCode::OK};
        Ok((status,Json(execute(&state,&actor,operation).await?)).into_response())
    }.await;
    result.unwrap_or_else(IntoResponse::into_response)
}
