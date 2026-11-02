//! Hosting as a plugin: apps from a container image, with port, memory, environment, logs and
//! power controls. The Core admits every call; this plugin checks again that an app belongs to the
//! invoking project before touching it.
pub mod placement;
pub mod store;
pub use webdock_plugin_kit::seal;

use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};
use uuid::Uuid;
use webdock_plugin_protocol::{ErrorCode, InvocationClaims, Manifest, PluginError};
use webdock_plugin_runtime::ActionHandler;

type Result<T> = std::result::Result<T, PluginError>;
pub const PLUGIN_ID: &str = "hosting";
const ACTIONS: &[(&str, &str)] = &[
    ("apps.create", "write"),
    ("apps.view", "read"),
    ("apps.logs", "write"),
    ("apps.deploy", "write"),
    ("apps.env", "write"),
    ("apps.power", "write"),
    ("apps.resize", "write"),
    ("apps.domains", "write"),
    ("apps.delete", "write"),
];
pub const MEMORY_MB: &[u32] = &[128, 256, 512, 1024, 2048];
fn invalid(message: &str) -> PluginError {
    PluginError::new(ErrorCode::InvalidInput, message)
}
fn missing() -> PluginError {
    PluginError::new(ErrorCode::NotFound, "App not found")
}
fn unavailable(context: &str, error: impl std::fmt::Display) -> PluginError {
    eprintln!("hosting_plugin_{context}: {error}");
    PluginError::new(ErrorCode::Unavailable, "Hosting service is unavailable")
}
fn store_error(error: store::Error) -> PluginError {
    unavailable("store", error)
}
fn parse<T: for<'a> Deserialize<'a>>(input: Value) -> Result<T> {
    serde_json::from_value(input).map_err(|_| invalid("Invalid app request"))
}

/// A registry reference such as `nginx:1.29-alpine` or `ghcr.io/org/app@sha256:…`.
pub fn image_reference(raw: &str, require_digest: bool) -> Result<String> {
    let image = raw.trim();
    let (name, digest) = image
        .split_once('@')
        .map_or((image, None), |(n, d)| (n, Some(d)));
    let valid_digest = digest.is_none_or(|d| {
        d.strip_prefix("sha256:").is_some_and(|h| {
            h.len() == 64
                && h.bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
    });
    if image.is_empty()
        || image.len() > 512
        || name.starts_with(['-', '.', '/', ':'])
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"./:_-".contains(&b))
        || name.contains("//")
        || !valid_digest
        || (require_digest && digest.is_none())
    {
        return Err(invalid(if require_digest {
            "Use an image pinned by digest (name@sha256:…)"
        } else {
            "Invalid image reference"
        }));
    }
    Ok(image.into())
}
/// `KEY=value` lines; blank lines and `#` comments are skipped. Keys follow shell rules.
pub fn environment(raw: &str) -> Result<Vec<(String, String)>> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for line in raw
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
    {
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| invalid("Write each variable as KEY=value"))?;
        let key = key.trim();
        if key.is_empty()
            || key.len() > 128
            || key.starts_with(|c: char| c.is_ascii_digit())
            || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            || value.chars().any(|c| c.is_control() && c != '\t')
            || !seen.insert(key.to_owned())
        {
            return Err(invalid("Invalid or repeated variable name"));
        }
        out.push((key.to_owned(), value.to_owned()));
    }
    if out.len() > 100 {
        return Err(invalid("At most 100 variables"));
    }
    Ok(out)
}
fn display_name(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 63 || value.chars().any(char::is_control) {
        return Err(invalid("Invalid app name"));
    }
    Ok(value.into())
}
fn port(value: i64) -> Result<u16> {
    u16::try_from(value)
        .ok()
        .filter(|p| *p >= 1)
        .ok_or_else(|| invalid("Invalid port"))
}
fn memory(value: i64) -> Result<u32> {
    u32::try_from(value)
        .ok()
        .filter(|m| MEMORY_MB.contains(m))
        .ok_or_else(|| invalid("Invalid memory size"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewApp {
    name: String,
    image: String,
    port: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    resource_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Deploy {
    resource_id: String,
    image: String,
    port: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Env {
    resource_id: String,
    env: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Power {
    resource_id: String,
    action: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Resize {
    resource_id: String,
    memory_mb: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Domains {
    resource_id: String,
    domains: String,
}
/// Own domains, one per line: full lower-case names, at most 10, none under the platform's domain.
/// Core refuses any the customer has not verified.
pub fn domains(raw: &str, platform: &str) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for line in raw.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let name = line.trim_end_matches('.').to_ascii_lowercase();
        let label = |l: &str| {
            (1..=63).contains(&l.len())
                && l.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                && !l.starts_with('-')
                && !l.ends_with('-')
        };
        if name.len() > 253 || name.split('.').count() < 2 || !name.split('.').all(label) {
            return Err(invalid(&format!("{line} is not a domain name")));
        }
        if name == platform || name.ends_with(&format!(".{platform}")) {
            return Err(invalid(&format!("{line} belongs to the platform")));
        }
        if !out.contains(&name) {
            out.push(name);
        }
    }
    if out.len() > 10 {
        return Err(invalid("An app answers on at most 10 own domains"));
    }
    Ok(out)
}
/// New apps start with this much memory; changing it is the separate `resize-apps` feature.
pub const DEFAULT_MEMORY_MB: i32 = 256;

pub struct Settings {
    /// Production requires digest-pinned images; local dev accepts tags.
    pub require_digest: bool,
}
pub struct Plugin {
    store: store::Store,
    sealer: seal::Sealer,
    placement: placement::Placement,
    settings: Settings,
    busy: Mutex<HashSet<Uuid>>,
}
/// Exclusive use of one app: every change, power action, deployment and delete holds it, so
/// they never interleave. Released when dropped.
pub struct Slot {
    plugin: Arc<Plugin>,
    id: Uuid,
}
impl Drop for Slot {
    fn drop(&mut self) {
        if let Ok(mut busy) = self.plugin.busy.lock() {
            busy.remove(&self.id);
        }
    }
}
fn busy() -> PluginError {
    PluginError::new(
        ErrorCode::Conflict,
        "The app is busy with another change; try again shortly",
    )
}
impl Plugin {
    pub fn new(
        store: store::Store,
        sealer: seal::Sealer,
        placement: placement::Placement,
        settings: Settings,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            sealer,
            placement,
            settings,
            busy: Mutex::new(HashSet::new()),
        })
    }
    fn claim(self: &Arc<Self>, id: Uuid) -> Result<Slot> {
        if !self
            .busy
            .lock()
            .map_err(|_| unavailable("lock", "poisoned"))?
            .insert(id)
        {
            return Err(busy());
        }
        Ok(Slot {
            plugin: Arc::clone(self),
            id,
        })
    }
    fn env(&self, app: &store::App) -> Result<Vec<(String, String)>> {
        let raw = self
            .sealer
            .open(&app.id, &app.env_sealed)
            .ok_or_else(|| unavailable("unseal", app.id))?;
        environment(&raw)
    }
    async fn target(&self, claims: &InvocationClaims, id: &Uuid) -> Result<store::App> {
        self.store
            .get(&claims.project_id, id)
            .await
            .map_err(store_error)?
            .ok_or_else(missing)
    }
    /// Hands the app as stored to the cluster. The agent applies it within seconds.
    async fn deploy(&self, app: &store::App) -> Result<()> {
        let env = self.env(app)?;
        let spec = placement::Spec {
            id: &app.id,
            project: &app.project_id,
            name: &app.name,
            image: &app.image,
            port: u16::try_from(app.port).map_err(|_| invalid("Invalid port"))?,
            memory_mb: u32::try_from(app.memory_mb).map_err(|_| invalid("Invalid memory"))?,
            env: &env,
            running: app.running,
            restart: &app.restart,
            domains: &app.domains,
        };
        self.placement.deploy(&spec).await
    }
    async fn create(self: &Arc<Self>, claims: &InvocationClaims, input: Value) -> Result<Value> {
        let input: NewApp = parse(input)?;
        let id = Uuid::new_v4();
        let app = store::App {
            id,
            project_id: claims.project_id.clone(),
            name: display_name(&input.name)?,
            image: image_reference(&input.image, self.settings.require_digest)?,
            port: i32::from(port(input.port)?),
            memory_mb: DEFAULT_MEMORY_MB,
            env_sealed: self.sealer.seal(&id, ""),
            running: true,
            restart: String::new(),
            domains: Vec::new(),
            created_at: String::new(),
        };
        let _slot = self.claim(id)?;
        self.store.insert(&app).await.map_err(store_error)?;
        if let Err(error) = self.deploy(&app).await {
            let _ = self.store.delete(&id).await;
            return Err(error);
        }
        Ok(json!({"resource":{"id":id.to_string(),"name":app.name}}))
    }
    async fn view(&self, claims: &InvocationClaims, input: Value) -> Result<Value> {
        let target: Target = parse(input)?;
        let app = self.target(claims, &uuid(&target.resource_id)?).await?;
        let observed = self.placement.observe(&app.id, app.running).await?;
        let url = (observed.status == "running").then(|| self.placement.url(&app.id, &app.name));
        let keys = self
            .env(&app)?
            .into_iter()
            .map(|(k, _)| k)
            .collect::<Vec<_>>();
        Ok(json!({
            "name": app.name, "status": observed.status, "image": app.image, "port": app.port, "memoryMb": app.memory_mb,
            "url": url, "env": keys, "error": observed.error, "createdAt": app.created_at,
            "host": self.placement.host(&app.id, &app.name), "domains": app.domains,
        }))
    }
    /// Claims the app, applies `edit` to the row as stored now, saves it and hands it to the cluster.
    /// If Core refuses the change (a plan limit, an unverified host), the row goes back as it was.
    async fn change(
        self: &Arc<Self>,
        claims: &InvocationClaims,
        id: &str,
        edit: impl FnOnce(&mut store::App) -> Result<()>,
    ) -> Result<Value> {
        let id = uuid(id)?;
        let _slot = self.claim(id)?;
        let before = self.target(claims, &id).await?;
        let mut app = before.clone();
        edit(&mut app)?;
        self.store.update(&app).await.map_err(store_error)?;
        if let Err(error) = self.deploy(&app).await {
            self.store.update(&before).await.map_err(store_error)?;
            return Err(error);
        }
        Ok(json!({"status":"deploying"}))
    }
    async fn delete(self: &Arc<Self>, claims: &InvocationClaims, input: Value) -> Result<Value> {
        let target: Target = parse(input)?;
        let id = uuid(&target.resource_id)?;
        let _slot = self.claim(id)?;
        // The Core proved ownership; an app already gone here is simply done.
        let Some(app) = self
            .store
            .get(&claims.project_id, &id)
            .await
            .map_err(store_error)?
        else {
            return Ok(json!({}));
        };
        self.placement.remove(&app.id).await?;
        self.store.delete(&app.id).await.map_err(store_error)?;
        Ok(json!({}))
    }
}
fn uuid(raw: &str) -> Result<Uuid> {
    Uuid::parse_str(raw).map_err(|_| invalid("Invalid app"))
}

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
            return Err(invalid("Invalid app request"));
        }
        let plugin = &self.0;
        match claims.action.as_str() {
            "apps.create" => plugin.create(claims, input).await,
            "apps.view" => plugin.view(claims, input).await,
            "apps.logs" => {
                let target: Target = parse(input)?;
                let app = plugin.target(claims, &uuid(&target.resource_id)?).await?;
                let observed = plugin.placement.observe(&app.id, app.running).await?;
                Ok(json!({"lines": observed.logs}))
            }
            "apps.deploy" => {
                let input: Deploy = parse(input)?;
                let image = image_reference(&input.image, plugin.settings.require_digest)?;
                let port = i32::from(port(input.port)?);
                plugin
                    .change(claims, &input.resource_id, |app| {
                        app.image = image;
                        app.port = port;
                        Ok(())
                    })
                    .await
            }
            "apps.env" => {
                let input: Env = parse(input)?;
                environment(&input.env)?;
                plugin
                    .change(claims, &input.resource_id, |app| {
                        app.env_sealed = plugin.sealer.seal(&app.id, &input.env);
                        Ok(())
                    })
                    .await
            }
            "apps.resize" => {
                let input: Resize = parse(input)?;
                let memory = i32::try_from(memory(input.memory_mb)?)
                    .map_err(|_| invalid("Invalid memory size"))?;
                plugin
                    .change(claims, &input.resource_id, |app| {
                        app.memory_mb = memory;
                        Ok(())
                    })
                    .await
            }
            "apps.domains" => {
                let input: Domains = parse(input)?;
                let domains = domains(&input.domains, plugin.placement.domain())?;
                plugin
                    .change(claims, &input.resource_id, |app| {
                        app.domains = domains;
                        Ok(())
                    })
                    .await
            }
            "apps.power" => {
                let input: Power = parse(input)?;
                let action = input.action;
                if !["start", "stop", "restart"].contains(&action.as_str()) {
                    return Err(invalid("Unknown power action"));
                }
                plugin
                    .change(claims, &input.resource_id, |app| {
                        match action.as_str() {
                            "start" => app.running = true,
                            "stop" => app.running = false,
                            _ => app.restart = Uuid::new_v4().to_string(),
                        }
                        Ok(())
                    })
                    .await
            }
            "apps.delete" => plugin.delete(claims, input).await,
            _ => Err(PluginError::new(
                ErrorCode::NotSupported,
                "Hosting action unavailable",
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
        return Err(invalid("Invalid hosting manifest"));
    }
    Ok(())
}
