//! Initial installation and recovery. Every external action happens after these transactions end.
use crate::authority::*;
use crate::{
    bindings::RuntimeBinding,
    catalog::{Catalog, CatalogDescriptor},
};
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, QueryResult, Statement};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};
use webdock_agent_protocol::plugin::{
    PluginRuntimeAction, PluginRuntimeCheckpoint, PluginRuntimeClaim, PluginRuntimeCompletion,
    PluginRuntimeLease, PluginRuntimeMaterial, PluginRuntimeOutcome, PluginRuntimeSpec,
};
use webdock_browser_session::StoredAuthority;
use webdock_hosting::AgentIdentity;
use webdock_secrets::SecretCipher;

pub(crate) struct InstallationConfig {
    pub catalog: Arc<Catalog>,
    pub cipher: SecretCipher,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallRequest {
    #[serde(rename = "catalogID")]
    pub catalog_id: String,
    pub version: String,
    pub review_sha256: String,
    #[serde(rename = "targetID")]
    pub target_id: String,
    pub configuration: Value,
    pub approved_capabilities: Vec<String>,
    pub idempotency_key: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateRequest {
    pub version: String,
    pub review_sha256: String,
    pub expected_revision: String,
    pub approved_capabilities: Vec<String>,
    pub idempotency_key: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RollbackRequest {
    pub release_generation: String,
    pub review_sha256: String,
    pub expected_revision: String,
    pub approved_capabilities: Vec<String>,
    pub idempotency_key: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallSnapshot {
    pub id: String,
    #[serde(rename = "pluginID")]
    pub plugin_id: String,
    pub kind: &'static str,
    pub status: OperationStatus,
    pub phase: String,
    pub generation: String,
    pub cursor: String,
    pub condition: Option<String>,
}
pub struct InstallClaim {
    id: String,
    plugin: String,
    generation: i64,
    token: uuid::Uuid,
    phase: String,
}
impl InstallClaim {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn plugin(&self) -> &str {
        &self.plugin
    }
    pub fn phase(&self) -> &str {
        &self.phase
    }
    pub fn generation(&self) -> u64 {
        self.generation as u64
    }
}
pub struct InstallWork {
    pub kind: String,
    pub source_runtime_generation: Option<u64>,
    pub plugin_id: String,
    pub platform_id: String,
    pub runtime_generation: u64,
    pub configuration_revision: u64,
    pub configuration: Value,
    pub manifest_bytes: String,
    pub manifest_sha256: String,
    pub image: String,
    pub target_id: String,
    pub cluster_id: String,
    pub target_policy: Value,
    pub recipe: Value,
    pub components: BTreeMap<String, String>,
}
pub struct PreparedInstallation {
    pub spec: Value,
    pub files: BTreeMap<String, String>,
    pub binding: RuntimeBinding,
    pub endpoint: Value,
}
fn error() -> PluginError {
    invalid("Invalid installation contract")
}
fn json<T: Serialize>(value: &T) -> Result<Value> {
    serde_json::to_value(value).map_err(|_| error())
}
fn parsed<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|_| error())
}
fn idempotency(value: &str) -> bool {
    !value.is_empty() && value.encode_utf16().count() <= 128
}
fn snapshot(row: &QueryResult) -> Result<InstallSnapshot> {
    let checkpoint: Value = row.try_get("", "checkpoint").map_err(db_error)?;
    Ok(InstallSnapshot {
        id: row.try_get("", "id").map_err(db_error)?,
        plugin_id: row.try_get("", "plugin_id").map_err(db_error)?,
        kind: operation_kind(
            row.try_get::<Option<String>>("", "kind")
                .ok()
                .flatten()
                .as_deref(),
        ),
        status: parsed(json!(
            row.try_get::<String>("", "status").map_err(db_error)?
        ))?,
        phase: row.try_get("", "phase").map_err(db_error)?,
        generation: row
            .try_get::<i64>("", "generation")
            .map_err(db_error)?
            .to_string(),
        cursor: checkpoint["sequence"].as_u64().unwrap_or(0).to_string(),
        condition: checkpoint["condition"].as_str().map(str::to_owned),
    })
}
fn seal_component(
    cipher: &SecretCipher,
    plugin: &str,
    generation: u64,
    name: &str,
    value: &str,
) -> Result<Value> {
    if value.len() > 400000 {
        return Err(error());
    }
    let mut parts = Vec::new();
    let mut offset = 0;
    while offset < value.len() {
        let mut end = (offset + 4000).min(value.len());
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        parts.push(Value::String(
            cipher
                .seal(
                    &&value[offset..end],
                    &format!(
                        "plugin-install:{plugin}:{generation}:{name}:{}",
                        parts.len()
                    ),
                )
                .map_err(|_| unavailable())?,
        ));
        offset = end;
    }
    if parts.is_empty() {
        parts.push(Value::String(
            cipher
                .seal(
                    &"",
                    &format!("plugin-install:{plugin}:{generation}:{name}:0"),
                )
                .map_err(|_| unavailable())?,
        ));
    }
    Ok(Value::Array(parts))
}
pub(crate) fn open_component(
    cipher: &SecretCipher,
    plugin: &str,
    generation: u64,
    name: &str,
    value: &Value,
) -> Result<String> {
    let mut result = String::new();
    let parts = value
        .as_array()
        .filter(|p| p.len() <= 101)
        .ok_or_else(error)?;
    for (index, part) in parts.iter().enumerate() {
        let text: String = cipher
            .open(
                part.as_str().ok_or_else(error)?,
                &format!("plugin-install:{plugin}:{generation}:{name}:{index}"),
            )
            .map_err(|_| unavailable())?;
        result.push_str(&text);
    }
    if result.len() > 400000 {
        return Err(error());
    }
    Ok(result)
}

#[path = "installation/admission.rs"]
mod admission;
#[path = "installation/agent.rs"]
mod agent;
#[path = "installation/worker.rs"]
mod worker;

fn operation_kind(kind: Option<&str>) -> &'static str {
    match kind {
        Some("update" | "updateRetry") => "update",
        Some("rollback" | "rollbackRetry") => "rollback",
        _ => "install",
    }
}
fn lifecycle(v: &Value) -> bool {
    !v["execution"]["source_runtime_generation"].is_null()
}
fn readable(manifest: &Value, high: i64) -> bool {
    high > 0
        && manifest["readableSchema"]["min"]
            .as_i64()
            .is_some_and(|v| v <= high)
        && manifest["readableSchema"]["max"]
            .as_i64()
            .is_some_and(|v| v >= high)
}
#[path = "installation/releases.rs"]
mod releases;
