//! Managed PostgreSQL as a plugin: databases on shared servers and dedicated instances.
//! The Core admits every call (audience, access mode, resource ownership); this plugin checks
//! again that a resource belongs to the invoking project before touching it.
pub mod dedicated;
pub use webdock_plugin_kit::seal;
pub mod shared;
pub mod store;

use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;
use webdock_plugin_protocol::{ErrorCode, InvocationClaims, Manifest, PluginError};
use webdock_plugin_runtime::ActionHandler;

type Result<T> = std::result::Result<T, PluginError>;
pub const PLUGIN_ID: &str = "postgres";
const ACTIONS: &[(&str, &str)] = &[
    ("databases.create", "write"),
    ("instances.create", "write"),
    ("databases.view", "read"),
    ("databases.credentials", "write"),
    ("databases.rotate", "write"),
    ("databases.delete", "write"),
    ("instances.delete", "write"),
];
fn invalid() -> PluginError {
    PluginError::new(ErrorCode::InvalidInput, "Invalid database request")
}
fn missing() -> PluginError {
    PluginError::new(ErrorCode::NotFound, "Database not found")
}
fn unavailable(context: &str, error: impl std::fmt::Display) -> PluginError {
    eprintln!("postgres_plugin_{context}: {error}");
    PluginError::new(ErrorCode::Unavailable, "Database service is unavailable")
}
fn parse<T: for<'a> Deserialize<'a>>(input: Value) -> Result<T> {
    serde_json::from_value(input).map_err(|_| invalid())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewDatabase {
    name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewInstance {
    name: String,
    version: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    resource_id: String,
}
fn display_name(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 63 || value.chars().any(char::is_control) {
        return Err(invalid());
    }
    Ok(value.into())
}
fn generated(id: &Uuid, prefix: &str) -> String {
    format!("wd_{prefix}{}", &id.simple().to_string()[..16])
}

pub struct Plugin {
    store: store::Store,
    sealer: seal::Sealer,
    servers: Vec<shared::SharedServer>,
    placement: Option<dedicated::Placement>,
    /// Capacity checks, rotations and deletes run one at a time, so counts and stored passwords
    /// never race. ponytail: global; per-resource locks if it ever matters.
    serial: tokio::sync::Mutex<()>,
}
impl Plugin {
    pub fn new(
        store: store::Store,
        sealer: seal::Sealer,
        servers: Vec<shared::SharedServer>,
        placement: Option<dedicated::Placement>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            sealer,
            servers,
            placement,
            serial: tokio::sync::Mutex::new(()),
        })
    }
    /// Shared databases whose creation was interrupted, e.g. by a restart, are marked failed.
    pub async fn resume(&self) -> std::result::Result<(), store::Error> {
        for resource in self.store.creating().await? {
            if resource.offering == "shared" {
                self.store
                    .failed(&resource.id, "Creation was interrupted")
                    .await?;
            }
        }
        Ok(())
    }
    fn password(&self, resource: &store::Resource) -> Result<String> {
        self.sealer
            .open(&resource.id, &resource.password_sealed)
            .ok_or_else(|| unavailable("unseal", resource.id))
    }
    fn server(&self, resource: &store::Resource) -> Result<&shared::SharedServer> {
        let id = resource.server_id.as_deref().unwrap_or_default();
        self.servers
            .iter()
            .find(|s| s.id == id)
            .ok_or_else(|| unavailable("server", id))
    }
    fn placement(&self) -> Result<&dedicated::Placement> {
        self.placement.as_ref().ok_or_else(|| {
            PluginError::new(
                ErrorCode::NotSupported,
                "Dedicated instances are not available here",
            )
        })
    }
    async fn target(&self, claims: &InvocationClaims, input: Value) -> Result<store::Resource> {
        let target: Target = parse(input)?;
        let id = Uuid::parse_str(&target.resource_id).map_err(|_| invalid())?;
        self.store
            .get(&claims.project_id, &id)
            .await
            .map_err(|e| unavailable("store", e))?
            .ok_or_else(missing)
    }
    /// A dedicated instance's state is what the cluster's agent last saw.
    async fn current(&self, mut r: store::Resource) -> Result<store::Resource> {
        if r.offering == "dedicated" {
            let (state, error) = self.placement()?.observe(&r.id).await?;
            r.state = state.into();
            r.error = error;
        }
        Ok(r)
    }
    fn spec<'a>(r: &'a store::Resource, password: &'a str) -> dedicated::Spec<'a> {
        dedicated::Spec {
            id: &r.id,
            project: &r.project_id,
            version: r.version.as_deref().unwrap_or_default(),
            database: &r.database_name,
            role: &r.role_name,
            password,
        }
    }
    async fn create_shared(&self, claims: &InvocationClaims, input: Value) -> Result<Value> {
        let input: NewDatabase = parse(input)?;
        let name = display_name(&input.name)?;
        let _serial = self.serial.lock().await;
        let mut chosen = None;
        for server in &self.servers {
            let used = self
                .store
                .on_server(&server.id)
                .await
                .map_err(|e| unavailable("store", e))?;
            if used < i64::from(server.capacity) && chosen.is_none_or(|(_, n)| used < n) {
                chosen = Some((server, used));
            }
        }
        let (server, _) = chosen.ok_or_else(|| {
            PluginError::new(
                ErrorCode::Unavailable,
                "No shared server has room for another database",
            )
        })?;
        let id = Uuid::new_v4();
        let password = seal::password();
        let resource = store::Resource {
            id,
            project_id: claims.project_id.clone(),
            offering: "shared".into(),
            name: name.clone(),
            server_id: Some(server.id.clone()),
            database_name: generated(&id, "db_"),
            role_name: generated(&id, "u_"),
            password_sealed: self.sealer.seal(&id, &password),
            host: Some(server.host.clone()),
            port: Some(i32::from(server.port)),
            version: None,
            state: "creating".into(),
            error: None,
            created_at: String::new(),
        };
        self.store
            .insert(&resource)
            .await
            .map_err(|e| unavailable("store", e))?;
        if let Err(error) = server
            .create(&resource.database_name, &resource.role_name, &password)
            .await
        {
            let _ = self.store.delete(&id).await;
            return Err(unavailable("shared_create", error));
        }
        self.store
            .ready(&id, &server.host, i32::from(server.port))
            .await
            .map_err(|e| unavailable("store", e))?;
        Ok(json!({"resource":{"id":id.to_string(),"name":name}}))
    }
    async fn create_dedicated(&self, claims: &InvocationClaims, input: Value) -> Result<Value> {
        let input: NewInstance = parse(input)?;
        let name = display_name(&input.name)?;
        let placement = self.placement()?;
        dedicated::image(&input.version).ok_or_else(invalid)?;
        let id = Uuid::new_v4();
        let password = seal::password();
        let resource = store::Resource {
            id,
            project_id: claims.project_id.clone(),
            offering: "dedicated".into(),
            name: name.clone(),
            server_id: None,
            database_name: "app".into(),
            role_name: generated(&id, "u_"),
            password_sealed: self.sealer.seal(&id, &password),
            host: Some(placement.host(&id)),
            port: Some(i32::from(placement.port())),
            version: Some(input.version),
            state: "creating".into(),
            error: None,
            created_at: String::new(),
        };
        self.store
            .insert(&resource)
            .await
            .map_err(|e| unavailable("store", e))?;
        if let Err(error) = placement.deploy(&Self::spec(&resource, &password)).await {
            let _ = self.store.delete(&id).await;
            return Err(error);
        }
        Ok(json!({"resource":{"id":id.to_string(),"name":name}}))
    }
    fn view(&self, r: &store::Resource) -> Value {
        let server = r
            .server_id
            .as_deref()
            .and_then(|id| self.servers.iter().find(|s| s.id == id))
            .map(|s| s.label.clone());
        json!({
            "id": r.id.to_string(), "name": r.name, "offering": r.offering, "state": r.state,
            "host": r.host, "port": r.port, "database": r.database_name, "user": r.role_name,
            "version": r.version, "server": server, "error": r.error, "createdAt": r.created_at,
            "sslMode": match (&self.placement, r.offering.as_str()) {
                (Some(placement), "dedicated") => placement.ssl_mode(),
                _ => "disable",
            },
        })
    }
    fn uri(&self, r: &store::Resource, password: &str) -> Option<String> {
        let tls = match (&self.placement, r.offering.as_str()) {
            (Some(placement), "dedicated") if placement.ssl_mode() == "verify-full" => {
                "?sslmode=verify-full&sslrootcert=system"
            }
            (Some(_), "dedicated") => "?sslmode=require",
            _ => "",
        };
        Some(format!(
            "postgresql://{}:{password}@{}:{}/{}{tls}",
            r.role_name,
            r.host.as_deref()?,
            r.port?,
            r.database_name
        ))
    }
    async fn rotate(&self, claims: &InvocationClaims, input: Value) -> Result<Value> {
        let _serial = self.serial.lock().await;
        let r = self.current(self.target(claims, input).await?).await?;
        if r.state != "ready" {
            return Err(PluginError::new(
                ErrorCode::Conflict,
                "The database is not ready",
            ));
        }
        let new = seal::password();
        match r.offering.as_str() {
            // The admin can always set it again, so a lost store write is repaired by rotating once more.
            "shared" => self
                .server(&r)?
                .password(&r.role_name, &new)
                .await
                .map_err(|e| unavailable("shared_rotate", e))?,
            // Declarative: the operator applies the new Secret; rotating again repairs a lost write.
            _ => self.placement()?.deploy(&Self::spec(&r, &new)).await?,
        }
        self.store
            .password(&r.id, &self.sealer.seal(&r.id, &new))
            .await
            .map_err(|e| unavailable("store", e))?;
        Ok(json!({}))
    }
    async fn delete(
        &self,
        claims: &InvocationClaims,
        input: Value,
        offering: &str,
    ) -> Result<Value> {
        let _serial = self.serial.lock().await;
        // The Core proved ownership; a resource already gone here is simply done.
        let r = match self.target(claims, input).await {
            Err(error) if error.code == ErrorCode::NotFound => return Ok(json!({})),
            other => other?,
        };
        if r.offering != offering {
            return Err(missing());
        }
        if offering == "shared" && r.state == "creating" {
            return Err(PluginError::new(
                ErrorCode::Conflict,
                "The database is still being created",
            ));
        }
        match offering {
            "shared" => self
                .server(&r)?
                .drop(&r.database_name, &r.role_name)
                .await
                .map_err(|e| unavailable("shared_delete", e))?,
            _ => self.placement()?.remove(&r.id).await?,
        }
        self.store
            .delete(&r.id)
            .await
            .map_err(|e| unavailable("store", e))?;
        Ok(json!({}))
    }
}

/// The runtime handler; a thin wrapper so background work can hold the plugin.
pub struct Handler(pub Arc<Plugin>);
#[tonic::async_trait]
impl ActionHandler for Handler {
    async fn invoke(
        &self,
        claims: &InvocationClaims,
        _token: &str,
        input: Value,
        configuration: Value,
    ) -> Result<Value> {
        if configuration != json!({}) {
            return Err(invalid());
        }
        let plugin = &self.0;
        match claims.action.as_str() {
            "databases.create" => plugin.create_shared(claims, input).await,
            "instances.create" => plugin.create_dedicated(claims, input).await,
            "databases.view" => {
                Ok(plugin.view(&plugin.current(plugin.target(claims, input).await?).await?))
            }
            "databases.credentials" => {
                let r = plugin.current(plugin.target(claims, input).await?).await?;
                if r.state != "ready" {
                    return Err(PluginError::new(
                        ErrorCode::Conflict,
                        "The database is not ready",
                    ));
                }
                let password = plugin.password(&r)?;
                Ok(json!({"value": password, "uri": plugin.uri(&r, &password)}))
            }
            "databases.rotate" => plugin.rotate(claims, input).await,
            "databases.delete" => plugin.delete(claims, input, "shared").await,
            "instances.delete" => plugin.delete(claims, input, "dedicated").await,
            _ => Err(PluginError::new(
                ErrorCode::NotSupported,
                "Database action unavailable",
            )),
        }
    }
}

/// The runtime refuses to start with any manifest other than the reviewed contract.
pub fn validate_manifest(manifest: &Manifest) -> Result<()> {
    let value = manifest.as_json();
    if manifest.id() != PLUGIN_ID
        || value["capabilities"] != json!(["plugin.workloads"])
        || value["httpBindings"] != json!([])
        || value["actions"]
            .as_array()
            .is_none_or(|a| a.len() != ACTIONS.len())
        || !ACTIONS.iter().all(|(name, intent)| {
            manifest.action(name).is_some_and(|action| {
                action["intent"] == *intent && action["capabilities"] == json!([])
            })
        })
    {
        return Err(invalid());
    }
    Ok(())
}
