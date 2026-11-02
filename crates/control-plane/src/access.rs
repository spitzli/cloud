use crate::{
    auth,
    http::{ApiFailure, ServiceState, reject},
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use webdock_access::{AccessCommand, AccessService, TenantCommand};
use webdock_identity::Principal;

#[path = "access_allowances.rs"]
mod allowances;

pub fn protocol_path(path: &str) -> bool {
    ["/api/access", "/api/tenant", "/api/invitation"]
        .iter()
        .any(|prefix| path.starts_with(prefix))
        || path == "/api/sites"
}
pub fn routes() -> Router<Arc<ServiceState>> {
    let mut routes = Router::new();
    for prefix in ["/api/console", "/api"] {
        routes = routes
            .route(&format!("{prefix}/access"), get(directory).post(manage))
            .route(&format!("{prefix}/tenants"), get(tenants))
            .route(
                &format!("{prefix}/tenant/{{id}}"),
                get(tenant).post(change_tenant),
            )
            .route(&format!("{prefix}/sites"), get(sites))
            .route(&format!("{prefix}/invitations"), get(invitations))
            .route(
                &format!("{prefix}/invitation/{{id}}"),
                get(invitation).post(respond),
            );
    }
    routes
        .merge(allowances::routes())
        .route(
            "/api/console/tenant/{id}/projects",
            get(projects).post(create_project),
        )
        .route(
            "/api/console/tenant/{id}/domains",
            get(domains).post(add_domain),
        )
        .route(
            "/api/console/tenant/{id}/domains/{domain}",
            post(verify_domain).delete(remove_domain),
        )
        .route("/api/console/project/{id}", get(project))
        .route("/api/tenant-preview/{action}", post(preview))
        .route("/api/console/tenant-preview/{action}", post(preview))
        .route("/api/portal/studio", post(legacy))
}
pub(crate) fn failure(error: webdock_access::Error) -> ApiFailure {
    reject(
        StatusCode::from_u16(error.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        "access_error",
        if error.status() >= 500 {
            "Access service is unavailable."
        } else {
            match error {
                webdock_access::Error::Unauthorized => "Sign in and complete account setup.",
                webdock_access::Error::Forbidden => "You do not have access to this action.",
                webdock_access::Error::LastAdministrator => {
                    "Keep at least one active customer owner and administrator."
                }
                webdock_access::Error::ActivationRequired => {
                    "This account or organization is not available for local access."
                }
                webdock_access::Error::Conflict => {
                    "This record changed. Refresh and review it before trying again."
                }
                webdock_access::Error::NotFound => "Record unavailable.",
                webdock_access::Error::DomainTaken => {
                    "Another customer already verified this domain."
                }
                webdock_access::Error::DomainInUse => {
                    "Something still uses this domain. Remove it there first."
                }
                webdock_access::Error::ProjectLimit => {
                    "Your plan allows no more projects. Ask your provider for a larger plan."
                }
                _ => "Check the entered values.",
            }
        },
    )
}
async fn actor<'a>(
    state: &'a ServiceState,
    headers: &HeaderMap,
) -> Result<(&'a AccessService, Principal), ApiFailure> {
    let service = state
        .access
        .as_deref()
        .ok_or_else(|| failure(webdock_access::Error::Configuration))?;
    let actor = auth::resolve(state, headers)
        .await?
        .ok_or_else(|| failure(webdock_access::Error::Unauthorized))?;
    Ok((service, actor))
}
fn output(result: Result<Value, ApiFailure>) -> Response {
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => error.into_response(),
    }
}
#[derive(Deserialize, Default)]
struct Search {
    #[serde(default)]
    search: String,
}
async fn directory(
    State(s): State<Arc<ServiceState>>,
    h: HeaderMap,
    Query(q): Query<Search>,
) -> Response {
    output(
        async {
            let (a, p) = actor(&s, &h).await?;
            a.list_access_as(&p, &q.search).await.map_err(failure)
        }
        .await,
    )
}
async fn manage(
    State(s): State<Arc<ServiceState>>,
    h: HeaderMap,
    Json(command): Json<AccessCommand>,
) -> Response {
    output(
        async {
            let (a, p) = actor(&s, &h).await?;
            Ok(json!(
                a.manage_access_as(&p, command).await.map_err(failure)?
            ))
        }
        .await,
    )
}
async fn tenants(State(s): State<Arc<ServiceState>>, h: HeaderMap) -> Response {
    output(
        async {
            let (a, p) = actor(&s, &h).await?;
            Ok(json!(a.list_tenants_as(&p).await.map_err(failure)?))
        }
        .await,
    )
}
async fn tenant(
    State(s): State<Arc<ServiceState>>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    output(
        async {
            let (a, p) = actor(&s, &h).await?;
            a.get_tenant_as(&p, &id).await.map_err(failure)
        }
        .await,
    )
}
async fn change_tenant(
    State(s): State<Arc<ServiceState>>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(command): Json<TenantCommand>,
) -> Response {
    output(
        async {
            let (a, p) = actor(&s, &h).await?;
            Ok(json!(
                a.manage_tenant_as(&p, &id, command)
                    .await
                    .map_err(failure)?
            ))
        }
        .await,
    )
}
async fn projects(
    State(s): State<Arc<ServiceState>>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    output(
        async {
            let (a, p) = actor(&s, &h).await?;
            Ok(json!(a.list_projects_as(&p, &id).await.map_err(failure)?))
        }
        .await,
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewDomain {
    domain: String,
}
async fn domains(
    State(s): State<Arc<ServiceState>>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    output(
        async {
            let (a, p) = actor(&s, &h).await?;
            Ok(json!(a.list_domains_as(&p, &id).await.map_err(failure)?))
        }
        .await,
    )
}
async fn add_domain(
    State(s): State<Arc<ServiceState>>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(command): Json<NewDomain>,
) -> Response {
    output(
        async {
            let (a, p) = actor(&s, &h).await?;
            a.add_domain_as(&p, &id, &command.domain)
                .await
                .map_err(failure)
        }
        .await,
    )
}
/// Checks the domain's TXT record now.
async fn verify_domain(
    State(s): State<Arc<ServiceState>>,
    h: HeaderMap,
    Path((id, domain)): Path<(String, String)>,
) -> Response {
    output(
        async {
            let (a, p) = actor(&s, &h).await?;
            a.verify_domain_as(&p, &id, &domain).await.map_err(failure)
        }
        .await,
    )
}
async fn remove_domain(
    State(s): State<Arc<ServiceState>>,
    h: HeaderMap,
    Path((id, domain)): Path<(String, String)>,
) -> Response {
    output(
        async {
            let (a, p) = actor(&s, &h).await?;
            a.remove_domain_as(&p, &id, &domain)
                .await
                .map_err(failure)?;
            Ok(json!({}))
        }
        .await,
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewProject {
    name: String,
}
async fn create_project(
    State(s): State<Arc<ServiceState>>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(command): Json<NewProject>,
) -> Response {
    output(
        async {
            let (a, p) = actor(&s, &h).await?;
            a.create_project_as(&p, &id, &command.name)
                .await
                .map_err(failure)
        }
        .await,
    )
}
async fn project(
    State(s): State<Arc<ServiceState>>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    output(
        async {
            let (a, p) = actor(&s, &h).await?;
            a.get_project_as(&p, &id).await.map_err(failure)
        }
        .await,
    )
}
async fn sites(State(s): State<Arc<ServiceState>>, h: HeaderMap) -> Response {
    output(
        async {
            let (a, p) = actor(&s, &h).await?;
            Ok(json!(a.account_sites_as(&p).await.map_err(failure)?))
        }
        .await,
    )
}
async fn invitations(State(s): State<Arc<ServiceState>>, h: HeaderMap) -> Response {
    output(
        async {
            let (a, p) = actor(&s, &h).await?;
            Ok(json!(a.list_invitations_as(&p).await.map_err(failure)?))
        }
        .await,
    )
}
async fn invitation(
    State(s): State<Arc<ServiceState>>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    output(
        async {
            let (a, p) = actor(&s, &h).await?;
            a.get_invitation_as(&p, &id).await.map_err(failure)
        }
        .await,
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InvitationAnswer {
    accept: bool,
}
async fn respond(
    State(s): State<Arc<ServiceState>>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<InvitationAnswer>,
) -> Response {
    output(
        async {
            let (a, p) = actor(&s, &h).await?;
            a.respond_invitation_as(&p, &id, input.accept)
                .await
                .map_err(failure)
        }
        .await,
    )
}
async fn preview(
    State(s): State<Arc<ServiceState>>,
    h: HeaderMap,
    Path(action): Path<String>,
    Json(input): Json<Value>,
) -> Response {
    output(
        async {
            let (a, p) = actor(&s, &h).await?;
            match action.as_str() {
                "start" => {
                    a.start_customer_scope_as(
                        &p,
                        input["customerID"]
                            .as_str()
                            .ok_or_else(|| failure(webdock_access::Error::InvalidInput))?,
                        serde_json::from_value(
                            input.get("mode").cloned().unwrap_or(json!("read_only")),
                        )
                        .map_err(|_| failure(webdock_access::Error::InvalidInput))?,
                    )
                    .await
                }
                "exit" => a.exit_preview_as(&p).await,
                "current" => a.current_preview_as(&p).await,
                _ => Err(webdock_access::Error::NotFound),
            }
            .map_err(failure)
        }
        .await,
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyRequest {
    operation: String,
    #[serde(default)]
    args: Vec<Value>,
}
fn invalid() -> ApiFailure {
    failure(webdock_access::Error::InvalidInput)
}
fn legacy_tenant(mut value: Value) -> Result<TenantCommand, ApiFailure> {
    if value["action"] == "profile" && value.get("profile").is_none() {
        let object = value.as_object_mut().ok_or_else(invalid)?;
        object.remove("action");
        value = json!({"action":"profile","profile":object});
    }
    serde_json::from_value(value).map_err(|_| invalid())
}
fn legacy_access(mut value: Value) -> Result<AccessCommand, ApiFailure> {
    if value["action"] == "suspend"
        && let Some(blocked) = value["blocked"].as_str()
    {
        value["blocked"] = match blocked {
            "true" => json!(true),
            "false" => json!(false),
            _ => return Err(invalid()),
        };
    }
    serde_json::from_value(value).map_err(|_| invalid())
}
async fn legacy(
    State(s): State<Arc<ServiceState>>,
    h: HeaderMap,
    Json(input): Json<LegacyRequest>,
) -> Response {
    let result=async{
        let(a,p)=actor(&s,&h).await?;
        let expected=match input.operation.as_str(){"session"|"requireAccessOperator"|"accountSites"|"listTenants"|"listTenantInvitations"|"exitTenantPreview"=>0,"listAccess"|"manageAccess"|"getTenant"|"startTenantPreview"=>1,"manageTenant"=>2,_=>return Err(failure(webdock_access::Error::NotFound))};
        if input.args.len()!=expected && !(input.operation=="listAccess"&&input.args.is_empty()){return Err(invalid())}
        let text=||input.args.first().and_then(Value::as_str).ok_or_else(invalid);
        let value=match input.operation.as_str(){
            "session"=>{let mut user=auth::public_user(&p);let mut result=json!({"user":user,"session":{"id":p.session_id,"userId":p.user_id,"expiresAt":p.expires_at},"operator":p.is_operator()});if p.in_tenant_preview(){user["role"]=json!("user");result["user"]=user;result["preview"]=a.current_preview_as(&p).await.map_err(failure)?;}result},
            "requireAccessOperator"=>{a.require_operator_as(&p).await.map_err(failure)?;json!({"user":auth::public_user(&p)})},
            "listAccess"=>a.list_access_as(&p,input.args.first().and_then(Value::as_str).unwrap_or("")).await.map_err(failure)?,
            "manageAccess"=>json!(a.manage_access_as(&p,legacy_access(input.args[0].clone())?).await.map_err(failure)?),
            "accountSites"=>json!(a.account_sites_as(&p).await.map_err(failure)?),
            "listTenants"=>json!(a.list_tenants_as(&p).await.map_err(failure)?),
            "listTenantInvitations"=>json!(a.list_invitations_as(&p).await.map_err(failure)?),
            "getTenant"=>a.get_tenant_as(&p,text()?).await.map_err(failure)?,
            "manageTenant"=>json!(a.manage_tenant_as(&p,text()?,legacy_tenant(input.args[1].clone())?).await.map_err(failure)?),
            "startTenantPreview"=>a.start_preview_as(&p,text()?).await.map_err(failure)?,
            "exitTenantPreview"=>a.exit_preview_as(&p).await.map_err(failure)?,
            _=>return Err(failure(webdock_access::Error::NotFound)),
        };
        Ok(json!({"data":value}))
    }.await;
    output(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_form_commands_keep_their_wire_shape() {
        assert!(matches!(
            legacy_access(json!({"action":"suspend","user":"123","blocked":"true"})),
            Ok(AccessCommand::Suspend { blocked: true, .. })
        ));
        assert!(legacy_access(json!({"action":"suspend","user":"123","blocked":"yes"})).is_err());
        assert!(matches!(
            legacy_tenant(json!({"action":"profile","name":"Customer","customer_type":"company"})),
            Ok(TenantCommand::Profile { .. })
        ));
    }
}
