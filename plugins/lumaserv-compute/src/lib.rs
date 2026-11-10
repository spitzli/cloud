//! Scoped observation adapter. Provider credentials remain in Core's named HTTP binding.
use serde::Deserialize;
use serde_json::{Value, json};
use spitzli_plugin_protocol::{ErrorCode, InvocationClaims, Manifest, PluginError};
use spitzli_plugin_runtime::{ActionHandler, HostClient};
use std::collections::{BTreeMap, HashSet};

type Result<T> = std::result::Result<T, PluginError>;
fn invalid() -> PluginError {
    PluginError::new(ErrorCode::InvalidInput, "Invalid compute request")
}
fn unavailable() -> PluginError {
    PluginError::new(ErrorCode::Unavailable, "Compute observation unavailable")
}
fn identifier(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.hyphenated().to_string() == value)
}

pub struct Compute {
    host: HostClient,
    projects: BTreeMap<String, String>,
}
impl Compute {
    pub fn new(host: HostClient, projects: BTreeMap<String, String>) -> Result<Self> {
        if projects.is_empty()
            || projects.len() > 256
            || projects.iter().any(|(local, remote)| {
                local.is_empty()
                    || local.len() > 20
                    || !local.bytes().all(|b| b.is_ascii_digit())
                    || !identifier(remote)
            })
        {
            return Err(invalid());
        }
        // Sharing a provider project between tenants would make observation ambiguous.
        if projects.values().collect::<HashSet<_>>().len() != projects.len() {
            return Err(invalid());
        }
        Ok(Self { host, projects })
    }
    async fn get(&self, token: &str, path: String) -> Result<Value> {
        let value = self
            .host
            .call(
                token,
                "http",
                "request",
                json!({"binding":"compute","method":"GET","path":path}),
            )
            .await?;
        if value["status"] != 200 || value["body"]["success"] != true {
            return Err(unavailable());
        }
        value["body"].get("data").cloned().ok_or_else(unavailable)
    }
    async fn list(&self, token: &str, project: &str, page: u32) -> Result<Vec<Value>> {
        let data = self
            .get(
                token,
                format!("/compute/servers?filter[project_id]={project}&page={page}&page_size=50"),
            )
            .await?;
        let rows = data
            .as_array()
            .filter(|r| r.len() <= 50)
            .ok_or_else(unavailable)?;
        let mut seen = HashSet::new();
        rows.iter()
            .map(|row| {
                let server = server(row, project)?;
                if !seen.insert(server["id"].as_str().ok_or_else(unavailable)?.to_owned()) {
                    return Err(unavailable());
                }
                Ok(server)
            })
            .collect()
    }
}

// Read provider JSON selectively: never relay raw provider fields, errors or metadata.
fn server(row: &Value, project: &str) -> Result<Value> {
    if row["project_id"] != project {
        return Err(PluginError::denied());
    }
    let id = row["id"]
        .as_str()
        .filter(|v| identifier(v))
        .ok_or_else(unavailable)?;
    let name = row["name"]
        .as_str()
        .filter(|v| !v.is_empty() && v.len() <= 160 && !v.chars().any(char::is_control))
        .ok_or_else(unavailable)?;
    let state = row["state"]
        .as_str()
        .filter(|v| ["INSTALLATION", "RESTORE", "ONLINE", "OFFLINE"].contains(v))
        .ok_or_else(unavailable)?;
    Ok(json!({"id":id,"name":name,"state":state}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    #[serde(default = "first_page")]
    page: u32,
}
fn first_page() -> u32 {
    1
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ServerInput {
    #[serde(rename = "server")]
    server_id: String,
}
#[tonic::async_trait]
impl ActionHandler for Compute {
    async fn invoke(
        &self,
        claims: &InvocationClaims,
        token: &str,
        input: Value,
        configuration: Value,
    ) -> Result<Value> {
        if configuration != json!({}) {
            return Err(invalid());
        }
        let project = self
            .projects
            .get(&claims.project_id)
            .ok_or_else(PluginError::denied)?;
        match claims.action.as_str() {
            "servers.list" | "servers.panel" => {
                let page: Page = serde_json::from_value(input).map_err(|_| invalid())?;
                if !(1..=10_000).contains(&page.page) {
                    return Err(invalid());
                }
                let rows = self.list(token, project, page.page).await?;
                if claims.action == "servers.panel" {
                    Ok(json!({"version":2,"title":"Cloud servers","blocks":[
                        {"kind":"table","title":"Servers","columns":[{"key":"name","label":"Name"},{"key":"state","label":"State"},{"key":"id","label":"ID"}],"rows":rows},
                        {"kind":"form","title":"Page","fields":[{"name":"page","label":"Page","type":"number","required":true}],"action":{"id":"servers.panel","label":"Load page"}},
                        {"kind":"actions","actions":[{"id":"servers.panel","label":"Refresh"}]}
                    ]}))
                } else {
                    Ok(json!({"servers":rows,"page":page.page,"pageSize":50}))
                }
            }
            "servers.get" => {
                let input: ServerInput = serde_json::from_value(input).map_err(|_| invalid())?;
                if !identifier(&input.server_id) {
                    return Err(invalid());
                }
                let row = self
                    .get(token, format!("/compute/servers/{}", input.server_id))
                    .await?;
                let server = server(&row, project)?;
                if server["id"] != input.server_id {
                    return Err(PluginError::denied());
                }
                let status = self
                    .get(
                        token,
                        format!("/compute/servers/{}/status", input.server_id),
                    )
                    .await?;
                let online = status["online"].as_bool().ok_or_else(unavailable)?;
                Ok(json!({"server":server,"online":online}))
            }
            _ => Err(PluginError::new(
                ErrorCode::NotSupported,
                "Compute action unavailable",
            )),
        }
    }
}

/// Refuse an installed descriptor which claims unsupported writes or capabilities.
pub fn validate_manifest(manifest: &Manifest) -> Result<()> {
    let value = manifest.as_json();
    if manifest.id() != "lumaserv-compute"
        || value["capabilities"] != json!(["plugin.http.read"])
        || value["httpBindings"] != json!(["compute"])
        || value["configuration"]
            != json!({"type":"object","properties":{},"additionalProperties":false})
        || !["servers.list", "servers.get", "servers.panel"]
            .iter()
            .all(|name| {
                manifest.action(name).is_some_and(|action| {
                    action["intent"] == "read"
                        && action["capabilities"] == json!(["plugin.http.read"])
                })
            })
        || value["actions"].as_array().is_none_or(|a| a.len() != 3)
    {
        return Err(invalid());
    }
    manifest.require_action_only_lifecycle()?;
    manifest.require_host_services(&[("http", 1)])
}
