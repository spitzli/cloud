//! Managed runtime metadata is independent of HostService construction; legacy files remain explicit.
use crate::installation::open_component;
use crate::{
    bindings::{RuntimeBinding, RuntimeBindings},
    db_error, invalid,
    scope::HostScopes,
    unavailable,
};
use async_trait::async_trait;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use serde_json::Value;
use std::sync::Arc;
use webdock_plugin_protocol::Result;
use webdock_secrets::SecretCipher;
pub struct ManagedBindings {
    worker: DatabaseConnection,
    scopes: HostScopes,
    fallback: Arc<dyn RuntimeBindings>,
    cipher: Option<SecretCipher>,
}
impl ManagedBindings {
    pub async fn new(
        worker: DatabaseConnection,
        rls_secret: String,
        fallback: Arc<dyn RuntimeBindings>,
    ) -> Result<Self> {
        webdock_persistence::connection::verify_runtime_role(&worker, "webdock_worker_runtime")
            .await
            .map_err(|_| invalid("Restricted binding pool required"))?;
        Ok(Self {
            worker,
            scopes: HostScopes::new(rls_secret)?,
            fallback,
            cipher: None,
        })
    }
    pub fn with_encryption_secret(mut self, secret: &str) -> Result<Self> {
        self.cipher =
            Some(SecretCipher::new(secret).map_err(|_| invalid("Binding encryption required"))?);
        Ok(self)
    }
    async fn managed(&self, plugin: &str) -> Result<bool> {
        let tx = self.worker.begin().await.map_err(db_error)?;
        self.scopes.controller(&tx).await?;
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM webdock_auth.plugin_install_execution WHERE plugin_id=$1) AS managed",[plugin.into()])).await.map_err(db_error)?.ok_or_else(unavailable)?;
        let managed = row.try_get("", "managed").map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        Ok(managed)
    }
    async fn metadata(&self, plugin: &str, generation: Option<u64>) -> Result<Option<Value>> {
        if !crate::slug(plugin) || generation.is_some_and(|g| g == 0 || g > 9_007_199_254_740_991) {
            return Err(invalid("Invalid managed runtime"));
        }
        let tx = self.worker.begin().await.map_err(db_error)?;
        self.scopes.controller(&tx).await?;
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT to_jsonb(e) AS execution,p.current_generation,p.runtime AS installation_runtime,r.status FROM webdock_auth.plugin_install_execution e JOIN webdock_auth.plugin_installation p ON p.id=e.plugin_id JOIN webdock_auth.plugin_runtime r ON r.plugin_id=e.plugin_id AND r.generation=e.runtime_generation WHERE e.plugin_id=$1 AND e.runtime_generation=coalesce($2::bigint,p.current_generation)",[plugin.into(),generation.map(|g|g as i64).into()])).await.map_err(db_error)?;
        let result = match row {
            Some(row) => {
                let mut value: Value = row.try_get("", "execution").map_err(db_error)?;
                value["currentGeneration"] = serde_json::json!(
                    row.try_get::<Option<i64>>("", "current_generation")
                        .map_err(db_error)?
                );
                value["installationRuntime"] = serde_json::json!(
                    row.try_get::<String>("", "installation_runtime")
                        .map_err(db_error)?
                );
                value["runtimeStatus"] =
                    serde_json::json!(row.try_get::<String>("", "status").map_err(db_error)?);
                Some(value)
            }
            None => None,
        };
        tx.commit().await.map_err(db_error)?;
        Ok(result)
    }
    pub async fn managed_endpoint(&self, plugin: &str, generation: u64) -> Result<Option<Value>> {
        if generation == 0 || generation > 9_007_199_254_740_991 {
            return Err(invalid("Invalid managed runtime"));
        }
        let fallback = self.fallback.runtime(plugin).await?;
        let value = self.metadata(plugin, Some(generation)).await?;
        if fallback.is_some() && self.managed(plugin).await? {
            return Err(invalid("Managed and file runtime enrollment collide"));
        }
        let Some(value) = value else {
            return Ok(None);
        };
        if fallback.is_some() {
            return Err(invalid("Managed and file runtime enrollment collide"));
        }
        if value["spec"].is_null() || value["runtime_generation"].as_u64() != Some(generation) {
            return Ok(None);
        }
        let raw = open_component(
            self.cipher.as_ref().ok_or_else(unavailable)?,
            plugin,
            generation,
            "prepared.endpoint",
            &value["private_components"]["prepared.endpoint"],
        )?;
        serde_json::from_str(&raw)
            .map(Some)
            .map_err(|_| unavailable())
    }
}
#[async_trait]
impl RuntimeBindings for ManagedBindings {
    async fn runtime_from_enrollment(
        &self,
        plugin: &str,
        managed: Option<Value>,
    ) -> Result<Option<RuntimeBinding>> {
        let fallback = self.fallback.runtime(plugin).await?;
        match managed {
            Some(value) if fallback.is_none() => serde_json::from_value(value)
                .map(Some)
                .map_err(|_| unavailable()),
            Some(_) => Err(invalid("Managed and file runtime enrollment collide")),
            None => Ok(fallback),
        }
    }
    async fn runtime(&self, plugin: &str) -> Result<Option<RuntimeBinding>> {
        let fallback = self.fallback.runtime(plugin).await?;
        if fallback.is_some() && self.managed(plugin).await? {
            return Err(invalid("Managed and file runtime enrollment collide"));
        }
        let Some(value) = self.metadata(plugin, None).await? else {
            return Ok(fallback);
        };
        if fallback.is_some() {
            return Err(invalid("Managed and file runtime enrollment collide"));
        }
        if value["currentGeneration"] != value["runtime_generation"]
            || value["installationRuntime"] != "ready"
            || !["ready", "draining"].contains(&value["runtimeStatus"].as_str().unwrap_or(""))
        {
            return Ok(None);
        }
        serde_json::from_value(value["binding"].clone())
            .map(Some)
            .map_err(|_| unavailable())
    }
}
