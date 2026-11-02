pub mod diagnostics;
pub type Result<T> = std::result::Result<T, &'static str>;
pub mod model;
pub const MAX_BYTES: u64 = 32 * 1024 * 1024;
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
pub mod backend;
pub mod engine;
pub mod producer;
pub mod sources;
pub mod store;
pub use engine::Engine;

use serde_json::{Value, json};
use std::sync::Arc;
use webdock_plugin_protocol::{ErrorCode, InvocationClaims, Manifest, PluginError};
use webdock_plugin_runtime::{ActionHandler, HostClient, JobHandler};

pub fn validate_manifest(manifest: &Manifest) -> std::result::Result<(), PluginError> {
    let mut actual = manifest.as_json().clone();
    let mut expected: Value = serde_json::from_slice(include_bytes!("../manifest.json"))
        .expect("compiled archive manifest");
    actual
        .as_object_mut()
        .expect("validated manifest")
        .remove("image");
    expected
        .as_object_mut()
        .expect("compiled manifest")
        .remove("image");
    if actual != expected {
        return Err(PluginError::denied());
    }
    Ok(())
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    resource_id: uuid::Uuid,
}
fn target(input: Value) -> std::result::Result<uuid::Uuid, PluginError> {
    serde_json::from_value::<Target>(input)
        .map(|v| v.resource_id)
        .map_err(|_| PluginError::denied())
}
fn failure(message: &'static str) -> PluginError {
    PluginError::new(ErrorCode::Conflict, message)
}
pub struct Handler {
    pub engine: Arc<Engine>,
    pub host: HostClient,
}
#[tonic::async_trait]
impl ActionHandler for Handler {
    async fn authorize_storage(
        &self,
        a: &webdock_plugin_protocol::storage::StorageAuthorization,
    ) -> std::result::Result<(), PluginError> {
        self.engine
            .authorize(a)
            .await
            .map_err(|_| PluginError::denied())?;
        let id = a.resource_id.parse().map_err(|_| PluginError::denied())?;
        let source = self
            .engine
            .core_source(&a.customer_id, &a.project_id, id)
            .await
            .map_err(|_| PluginError::denied())?;
        let status = self
            .engine
            .status(&a.customer_id, &a.project_id, id)
            .await
            .map_err(|_| PluginError::denied())?;
        let job = status["jobID"].as_str().ok_or_else(PluginError::denied)?;
        sources::Sources::new(self.host.channel())
            .validate_download(a, source.as_ref(), job)
            .await
            .map_err(|_| PluginError::denied())?;
        self.engine
            .authorize(a)
            .await
            .map_err(|_| PluginError::denied())
    }
    async fn invoke(
        &self,
        c: &InvocationClaims,
        token: &str,
        input: Value,
        configuration: Value,
    ) -> std::result::Result<Value, PluginError> {
        if c.plugin_id != "archives" || c.job.is_some() || configuration != json!({}) {
            return Err(PluginError::denied());
        }
        let sources = sources::Sources::new(self.host.channel());
        if c.action == "archives.sources" {
            if input != json!({}) {
                return Err(PluginError::denied());
            }
            return sources.choices(token).await.map_err(failure);
        }
        if c.action == "archives.create" {
            let source = if let Some(id) = input["source_id"].as_str() {
                Some(
                    sources
                        .bind(
                            token,
                            input["kind"].as_str().ok_or_else(PluginError::denied)?,
                            id,
                        )
                        .await
                        .map_err(failure)?,
                )
            } else {
                None
            };
            return self
                .engine
                .create_bound(&c.customer_id, &c.project_id, input, source)
                .await
                .map_err(failure);
        }
        let id = target(input)?;
        match c.action.as_str() {
            "archives.generate" => {
                self.engine
                    .available(&c.customer_id, &c.project_id, id)
                    .await
                    .map_err(failure)?;
                if let Some(source) = self
                    .engine
                    .core_source(&c.customer_id, &c.project_id, id)
                    .await
                    .map_err(failure)?
                {
                    sources.validate(token, &source).await.map_err(failure)?;
                }
                let queued=self.host.call(token,"jobs","enqueue",json!({"name":"archives.produce","payload":{"resource_id":id},"idempotencyKey":format!("archive-{id}"),"attempts":3})).await?;
                self.engine
                    .attach_job(
                        &c.customer_id,
                        &c.project_id,
                        id,
                        queued["id"].as_str().ok_or_else(PluginError::denied)?,
                    )
                    .await
                    .map_err(failure)?;
                Ok(queued)
            }
            "archives.status" => {
                let mut status = self
                    .engine
                    .status(&c.customer_id, &c.project_id, id)
                    .await
                    .map_err(failure)?;
                if let Some(job) = status["jobID"].as_str() {
                    status["job"] = self
                        .host
                        .call(token, "jobs", "get", json!({"id":job}))
                        .await?;
                }
                Ok(status)
            }
            "archives.download" => {
                let status = self
                    .engine
                    .status(&c.customer_id, &c.project_id, id)
                    .await
                    .map_err(failure)?;
                let job = status["jobID"].as_str().ok_or_else(PluginError::denied)?;
                let completed = self
                    .host
                    .call(token, "jobs", "get", json!({"id":job}))
                    .await?;
                if completed["state"] != "completed" {
                    return Err(PluginError::new(
                        ErrorCode::Conflict,
                        "Export job has not completed",
                    ));
                }
                if let Some(source) = self
                    .engine
                    .core_source(&c.customer_id, &c.project_id, id)
                    .await
                    .map_err(failure)?
                {
                    sources.validate(token, &source).await.map_err(failure)?;
                }
                serde_json::to_value(
                    self.engine
                        .plan(&c.customer_id, &c.project_id, id)
                        .await
                        .map_err(failure)?,
                )
                .map_err(|_| PluginError::denied())
            }
            "archives.delete" => {
                let status = self
                    .engine
                    .status(&c.customer_id, &c.project_id, id)
                    .await
                    .map_err(failure)?;
                if let Some(job) = status["jobID"].as_str() {
                    let state = self
                        .host
                        .call(token, "jobs", "get", json!({"id":job}))
                        .await?;
                    if ["waiting", "active"].contains(&state["state"].as_str().unwrap_or("")) {
                        self.host
                            .call(token, "jobs", "cancel", json!({"id":job}))
                            .await?;
                    }
                }
                self.engine
                    .delete(&c.customer_id, &c.project_id, id)
                    .await
                    .map_err(failure)?;
                Ok(json!({}))
            }
            _ => Err(PluginError::denied()),
        }
    }
}
#[tonic::async_trait]
impl JobHandler for Handler {
    async fn run(
        &self,
        c: &InvocationClaims,
        token: &str,
        input: Value,
        configuration: Value,
    ) -> std::result::Result<Value, PluginError> {
        if c.plugin_id != "archives" || c.action != "archives.produce" || configuration != json!({})
        {
            return Err(PluginError::denied());
        }
        let job = c.job.as_ref().ok_or_else(PluginError::denied)?;
        let seconds = c.exp.saturating_sub(now()).saturating_sub(2).min(35);
        if seconds < 2 {
            return Err(PluginError::new(
                ErrorCode::DeadlineExceeded,
                "Export attempt budget exhausted",
            ));
        }
        let sources = sources::Sources::new(self.host.channel());
        tokio::time::timeout(
            std::time::Duration::from_secs(seconds as u64),
            self.engine.produce_bound(
                &c.customer_id,
                &c.project_id,
                target(input)?,
                &job.id,
                Some((&sources, token)),
            ),
        )
        .await
        .map_err(|_| {
            PluginError::new(
                ErrorCode::DeadlineExceeded,
                "Export attempt budget exhausted",
            )
        })?
        .map_err(|e| PluginError::new(ErrorCode::Unavailable, e))
    }
}

/// SDK and tonic enable multiple Rustls providers; choose before either creates a TLS client.
pub fn initialize_crypto() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

#[cfg(feature = "local-native-fixture")]
pub mod fixture;

/// Single private leaf only; existing paths are validated by Engine::connect.
pub fn prepare_staging(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    if !path.is_absolute()
        || path
            .parent()
            .is_none_or(|p| std::fs::canonicalize(p).ok().as_deref() != Some(p))
    {
        return Err("Canonical archive staging parent required");
    }
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(_) => Err("Private archive staging unavailable"),
    }
}
