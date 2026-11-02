//! Separately supervised initial-install orchestration. Privileged storage/issuer inputs stay here.
use super::*;
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use std::time::Duration;
use std::{
    collections::BTreeMap,
    path::{Path as FilePath, PathBuf},
};
use webdock_agent_protocol::plugin::{
    PluginRuntimeEgress, PluginRuntimeHostAlias, PluginRuntimeMaterial,
};
use webdock_plugin_host::provisioning::{
    storage::{MetadataProvisioner, StoragePlan},
    tls::issue_runtime_identity,
};
use webdock_plugin_host::{InstallClaim, InstallWork, PreparedInstallation};
use webdock_plugin_host::{PeerEnrollment, RuntimeBinding};
type Error = crate::plugin_config::ConfigurationError;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerConfig {
    storage_targets: Vec<StorageTarget>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StorageTarget {
    id: String,
    admin_url: String,
    authority: PathBuf,
    runtime_host: String,
    runtime_port: u16,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TargetPolicy {
    /// Explicit operator-approved binary destinations; never derived from action input.
    #[serde(default)]
    storage: BTreeMap<String, BTreeMap<String, webdock_plugin_http::storage::StorageBinding>>,
    runtime_domain: String,
    runtime_port: u16,
    entry_point: String,
    #[serde(default = "https_port")]
    runtime_origin_port: u16,
    #[serde(default)]
    runtime_ingress_host: Option<String>,
    core: Callback,
    runtime_identity: Issuer,
    #[serde(default, rename = "storageID")]
    storage_id: Option<String>,
    resources: Resources,
    egress: Vec<PluginRuntimeEgress>,
    #[serde(default)]
    host_aliases: Vec<PluginRuntimeHostAlias>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Callback {
    origin: String,
    domain: String,
    certificate: PathBuf,
    private_key: PathBuf,
    authority: PathBuf,
    server_certificate_sha256: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Issuer {
    authority: PathBuf,
    private_key: PathBuf,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Resources {
    cpu_millicores: u32,
    memory_bytes: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Recipe {
    #[serde(default)]
    private_configuration: serde_json::Map<String, Value>,
    #[serde(default)]
    storage: bool,
    #[serde(default)]
    sealing_key: bool,
    #[serde(default)]
    route_domains: Vec<String>,
}
pub struct InstallationWorker {
    config: WorkerConfig,
    platform_database: sea_orm::DatabaseConnection,
    invocation_key: PathBuf,
    kernel: KernelIdentity,
}
struct KernelIdentity {
    certificate: String,
    private_key: String,
    authority: String,
    fingerprint: String,
}

fn https_port() -> u16 {
    443
}
fn private_text(path: &FilePath) -> Result<String, Error> {
    let value = crate::config::read_private_file(path)?;
    String::from_utf8(value).map_err(|_| "Private installation file must be UTF8".into())
}
fn random_hex() -> Result<String, Error> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|_| "Installation material unavailable")?;
    Ok(hex::encode(bytes))
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, Error> {
    serde_json::from_value(value).map_err(|_| "Invalid private installation profile".into())
}
fn component<'a>(work: &'a InstallWork, name: &str) -> Result<&'a str, Error> {
    work.components
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| "Retained installation material incomplete".into())
}
fn seeds_needed(kind: &str, components: &BTreeMap<String, String>) -> Result<(bool, bool), Error> {
    if !["install", "update", "rollback"].contains(&kind) {
        return Err("Invalid private installation profile".into());
    }
    let masters = ["storagePassword", "sealingKey"]
        .iter()
        .filter(|key| components.contains_key(**key))
        .count();
    let identity = ["certificate", "privateKey", "certificateSha256"]
        .iter()
        .filter(|key| components.contains_key(**key))
        .count();
    if (masters != 2 && !(masters == 0 && kind == "install")) || ![0, 3].contains(&identity) {
        return Err("Retained installation material incomplete".into());
    }
    Ok((masters == 0, identity == 0))
}
fn guarded_origin(value: &str) -> Result<url::Url, Error> {
    let url = url::Url::parse(value).map_err(|_| "Invalid private callback")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Pinned HTTPS callback required".into());
    }
    Ok(url)
}
/// Observe the same restricted platform connection used by the worker; no cached/configured ID.
pub async fn platform_storage_identity(
    database: &sea_orm::DatabaseConnection,
) -> Result<webdock_plugin_host::provisioning::storage::ClusterIdentity, Error> {
    let row = database.query_one_raw(Statement::from_string(
        DbBackend::Postgres,
        "SELECT system_identifier::pg_catalog.text AS identity FROM pg_catalog.pg_control_system()",
    )).await.map_err(|_| "Platform database identity unavailable")?
        .ok_or("Platform database identity unavailable")?;
    let value = row
        .try_get("", "identity")
        .map_err(|_| "Platform database identity unavailable")?;
    Ok(webdock_plugin_host::provisioning::storage::ClusterIdentity::parse(value)?)
}
impl InstallationWorker {
    pub async fn load(
        path: &FilePath,
        platform_database: &sea_orm::DatabaseConnection,
        plugin: &crate::plugin_config::PluginConfig,
    ) -> Result<Self, Error> {
        let config: WorkerConfig = serde_json::from_slice(&crate::config::read_private_file(path)?)
            .map_err(|_| "Invalid private installer worker configuration")?;
        if config.storage_targets.len() > 32 {
            return Err("Too many private storage targets".into());
        }
        let mut names = std::collections::BTreeSet::new();
        for target in &config.storage_targets {
            let url =
                url::Url::parse(&target.admin_url).map_err(|_| "Invalid private storage target")?;
            if target.id.is_empty()
                || !names.insert(&target.id)
                || !target.authority.is_absolute()
                || target.runtime_host.is_empty()
                || target.runtime_port == 0
                || !["postgres", "postgresql"].contains(&url.scheme())
                || url.host_str().is_none()
                || url.query_pairs().any(|(key, _)| key != "sslmode")
                || url.fragment().is_some()
            {
                return Err("Invalid private storage target".into());
            }
        }
        platform_storage_identity(platform_database).await?;
        let platform_database = platform_database.clone();
        let (certificate, private_key, authority) = match &plugin.control {
            crate::plugin_config::ControlListener::MutualTls {
                certificate,
                private_key,
                authority,
                ..
            } => (
                private_text(certificate)?,
                private_text(private_key)?,
                private_text(authority)?,
            ),
            crate::plugin_config::ControlListener::Unix { .. } => {
                return Err("Managed installation requires the native mTLS Core listener".into());
            }
        };
        let (_, pem) = x509_parser::pem::parse_x509_pem(certificate.as_bytes())
            .map_err(|_| "Invalid Core identity")?;
        let fingerprint = hex::encode(Sha256::digest(&pem.contents));
        Ok(Self {
            config,
            platform_database,
            invocation_key: plugin.invocation_signing_key.clone(),
            kernel: KernelIdentity {
                certificate,
                private_key,
                authority,
                fingerprint,
            },
        })
    }
    pub async fn advance(&self, endpoints: &PluginEndpoints) -> Result<bool, Error> {
        let Some(claim) = endpoints.host.claim_installation(None).await? else {
            return Ok(false);
        };
        let result = self.execute(endpoints, &claim).await;
        if let Err(error) = result {
            // Never expose underlying connection/PKI/SQL errors in an operation or process log.
            let condition = failure_condition(error.as_ref());
            tracing::warn!(condition, "Installation requires reconciliation");
            if endpoints
                .host
                .fail_installation(&claim, condition, true)
                .await
                .is_err()
            {
                tracing::warn!(
                    "Installation authority changed; retained resources require reconciliation"
                );
            }
        }
        Ok(true)
    }
    async fn execute(
        &self,
        endpoints: &PluginEndpoints,
        claim: &InstallClaim,
    ) -> Result<(), Error> {
        if !endpoints.host.guard_installation(claim).await? {
            return Err("Installation authority changed".into());
        }
        let work = endpoints.host.install_work(claim).await?;
        if claim.phase() == "verifying_runtime" {
            let description = discover(endpoints, &work.plugin_id, work.runtime_generation, || {
                endpoints.host.guard_installation(claim)
            })
            .await?;
            endpoints
                .host
                .promote_installation(claim, description)
                .await?;
            return Ok(());
        }
        self.prepare(endpoints, claim, work).await
    }
    async fn prepare(
        &self,
        endpoints: &PluginEndpoints,
        claim: &InstallClaim,
        mut work: InstallWork,
    ) -> Result<(), Error> {
        let policy: TargetPolicy = decode(work.target_policy.clone())?;
        let recipe: Recipe = decode(work.recipe.clone())?;
        guarded_origin(&policy.core.origin)?;
        self.kernel.validate(&policy)?;
        validate_storage_bindings(&policy.storage)?;
        let reserved = [
            "manifest",
            "issuerPublicKeyHex",
            "generation",
            "listen",
            "listenTls",
            "core",
            "coreTls",
            "corePeers",
            "storageUrl",
            "storageCa",
            "secretKeyHex",
        ];
        if reserved
            .iter()
            .any(|key| recipe.private_configuration.contains_key(*key))
        {
            return Err("Private recipe overrides reserved runtime configuration".into());
        }
        let lifecycle = work.kind != "install";
        let namespace = if lifecycle {
            PluginRuntimeSpec::namespace_name_v2(
                &work.platform_id,
                &work.plugin_id,
                work.runtime_generation,
            )
        } else {
            PluginRuntimeSpec::namespace_name(&work.platform_id, &work.plugin_id)
        };
        let host = format!("{namespace}.{}", policy.runtime_domain);
        let (create_master, create_identity) = seeds_needed(&work.kind, &work.components)?;
        if create_master || create_identity {
            let mut seeds = work.components.clone();
            if create_master {
                seeds.insert("storagePassword".into(), random_hex()?);
                seeds.insert("sealingKey".into(), random_hex()?);
            }
            if create_identity {
                let identity = issue_runtime_identity(
                    &private_text(&policy.runtime_identity.authority)?,
                    &private_text(&policy.runtime_identity.private_key)?,
                    &host,
                )?;
                seeds.insert("certificate".into(), identity.certificate_pem);
                seeds.insert("privateKey".into(), identity.private_key_pem);
                seeds.insert("certificateSha256".into(), identity.certificate_sha256);
            }
            endpoints
                .host
                .persist_install_components(claim, seeds)
                .await?;
            work = endpoints.host.install_work(claim).await?;
        }
        let mut storage = None;
        if recipe.storage {
            let target = self
                .config
                .storage_targets
                .iter()
                .find(|t| Some(t.id.as_str()) == policy.storage_id.as_deref())
                .ok_or("Private storage target unavailable")?;
            let ca = private_text(&target.authority)?;
            let provisioner =
                MetadataProvisioner::connect(&target.admin_url, Some(ca.as_bytes())).await?;
            let platform = platform_storage_identity(&self.platform_database).await?;
            if let Err(error) = provisioner.require_separate_cluster(&platform).await {
                if error == webdock_plugin_host::provisioning::ProvisioningError::Ownership {
                    return Err("Plugin storage must be separate from platform PostgreSQL".into());
                }
                return Err(error.into());
            }
            if !endpoints.host.guard_installation(claim).await? {
                return Err("Installation authority changed".into());
            }
            let plan = StoragePlan::new(
                &work.platform_id,
                &work.plugin_id,
                component(&work, "storagePassword")?.into(),
                target.runtime_host.clone(),
                target.runtime_port,
            )?;
            let result = if lifecycle {
                provisioner.retained(&plan).await?
            } else {
                provisioner.provision(&plan).await?
            };
            storage = Some((result.url, ca));
        }
        if !endpoints.host.guard_installation(claim).await? {
            return Err("Installation authority changed".into());
        }
        let mut configuration = recipe.private_configuration;
        configuration.insert(
            "manifest".into(),
            json!("/run/webdock/runtime/manifest.json"),
        );
        configuration.insert(
            "issuerPublicKeyHex".into(),
            json!(hex::encode(
                crate::plugin_config::signing_key(&self.invocation_key)?
                    .verifying_key()
                    .to_bytes()
            )),
        );
        configuration.insert("generation".into(), json!(work.runtime_generation));
        configuration.insert(
            "corePeers".into(),
            json!([{"kind":"mutualTls","certificateSha256":self.kernel.fingerprint}]),
        );
        configuration.insert("listenTls".into(),json!({"address":format!("0.0.0.0:{}",policy.runtime_port),"certificate":"/run/webdock/runtime/runtime.crt","privateKey":"/run/webdock/runtime/runtime.key","authority":"/run/webdock/runtime/core-ca.crt"}));
        configuration.insert("coreTls".into(),json!({"origin":policy.core.origin,"domain":policy.core.domain,"certificate":"/run/webdock/runtime/runtime.crt","privateKey":"/run/webdock/runtime/runtime.key","authority":"/run/webdock/runtime/core-ca.crt","serverCertificateSha256":policy.core.server_certificate_sha256}));
        let core_ca = private_text(&policy.core.authority)?;
        let storage_ca = match storage {
            Some((url, ca)) => {
                configuration.insert("storageUrl".into(), json!(url));
                configuration.insert(
                    "storageCa".into(),
                    json!("/run/webdock/runtime/storage-ca.crt"),
                );
                ca
            }
            None => core_ca.clone(),
        };
        if recipe.sealing_key {
            configuration.insert(
                "secretKeyHex".into(),
                json!(component(&work, "sealingKey")?),
            );
        }
        let files: BTreeMap<String, String> = BTreeMap::from([
            (
                "runtime.json".into(),
                serde_json::to_string(&configuration)?,
            ),
            ("manifest.json".into(), work.manifest_bytes.clone()),
            (
                "runtime.crt".into(),
                component(&work, "certificate")?.into(),
            ),
            ("runtime.key".into(), component(&work, "privateKey")?.into()),
            ("core-ca.crt".into(), core_ca),
            ("storage-ca.crt".into(), storage_ca),
        ]);
        let material = PluginRuntimeMaterial {
            files: files.clone(),
        };
        let runtime_origin_port = policy.runtime_origin_port;
        let ingress_host = policy
            .runtime_ingress_host
            .as_deref()
            .unwrap_or(&host)
            .to_owned();
        let spec = PluginRuntimeSpec {
            version: if lifecycle { 2 } else { 1 },
            platform_id: work.platform_id.clone(),
            plugin_id: work.plugin_id.clone(),
            runtime_generation: work.runtime_generation,
            configuration_revision: work.configuration_revision,
            cluster_id: work.cluster_id.clone(),
            manifest_sha256: work.manifest_sha256.clone(),
            material_sha256: material.fingerprint()?,
            image: work.image.clone(),
            namespace: namespace.clone(),
            service: PluginRuntimeSpec::service_name().into(),
            runtime_host: host.clone(),
            port: policy.runtime_port,
            entry_point: policy.entry_point,
            cpu_millicores: policy.resources.cpu_millicores,
            memory_bytes: policy.resources.memory_bytes,
            egress: policy.egress,
            host_aliases: policy.host_aliases,
        };
        spec.validate()?;
        let pin = component(&work, "certificateSha256")?.to_owned();
        let binding = RuntimeBinding {
            storage: policy.storage,
            generation: work.runtime_generation,
            namespace,
            service: spec.service.clone(),
            credential_sha256: pin.clone(),
            peers: vec![PeerEnrollment::MutualTls {
                certificate_sha256: pin.clone(),
            }],
            public_configuration: BTreeMap::new(),
            projects: BTreeMap::new(),
            public_customer_configuration: BTreeMap::new(),
            customers: BTreeMap::new(),
            workload_cluster: Some(work.cluster_id),
            route_domains: recipe.route_domains,
        };
        let origin = format!("https://{ingress_host}:{runtime_origin_port}");
        guarded_origin(&origin)?;
        let endpoint = json!({"generation":work.runtime_generation,"transport":{"kind":"mutualTls","origin":origin,"domain":host,
            "certificate":policy.core.certificate,"privateKey":policy.core.private_key,"authority":policy.runtime_identity.authority,"serverCertificateSha256":pin}});
        endpoints
            .host
            .prepare_installation(
                claim,
                PreparedInstallation {
                    spec: serde_json::to_value(spec)?,
                    files,
                    binding,
                    endpoint,
                },
            )
            .await?;
        Ok(())
    }
}

fn failure_condition(error: &(dyn std::error::Error + Send + Sync + 'static)) -> &'static str {
    use webdock_plugin_host::provisioning::ProvisioningError;
    if let Some(error) = error.downcast_ref::<ProvisioningError>() {
        return match error {
            ProvisioningError::Trust => "private_provisioning_trust_rejected",
            ProvisioningError::Ownership => "private_storage_ownership_mismatch",
            ProvisioningError::Invalid => "private_provisioning_invalid",
            ProvisioningError::Unavailable => "private_provisioning_unavailable",
        };
    }
    if let Some(error) = error.downcast_ref::<PluginError>() {
        return match error.code {
            ErrorCode::PermissionDenied => "installation_authority_changed",
            ErrorCode::InvalidInput => "installation_contract_invalid",
            ErrorCode::Unavailable | ErrorCode::DeadlineExceeded => {
                "installation_service_unavailable"
            }
            _ => "installation_reconciliation_required",
        };
    }
    if error
        .downcast_ref::<webdock_agent_protocol::plugin::PluginRuntimeError>()
        .is_some()
    {
        return "runtime_spec_invalid";
    }
    if error.downcast_ref::<std::io::Error>().is_some() {
        return "private_file_unavailable";
    }
    match error.to_string().as_str() {
        "Invalid private installation profile" => "private_profile_invalid",
        "Unsupported managed kernel trust profile" => "kernel_trust_profile_mismatch",
        "Retained installation material incomplete" => "private_material_incomplete",
        "Private storage target unavailable" => "private_storage_target_unavailable",
        "Plugin storage must be separate from platform PostgreSQL" => {
            "private_storage_not_separate"
        }
        "Private recipe overrides reserved runtime configuration" => {
            "reserved_private_configuration"
        }
        _ => "installation_reconciliation_required",
    }
}

fn validate_storage_bindings(
    storage: &BTreeMap<String, BTreeMap<String, webdock_plugin_http::storage::StorageBinding>>,
) -> Result<(), Error> {
    if storage.len() > 256 {
        return Err("Too many project storage bindings".into());
    }
    for (project, aliases) in storage {
        if project.is_empty()
            || project.len() > 20
            || !project.bytes().all(|b| b.is_ascii_digit())
            || aliases.len() > 32
        {
            return Err("Invalid project storage bindings".into());
        }
        for (alias, binding) in aliases {
            if alias.is_empty()
                || alias.len() > 64
                || !alias
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            {
                return Err("Invalid storage binding alias".into());
            }
            let object = format!(
                "{}{}__binding_probe/object",
                binding.origin.trim_end_matches('/'),
                binding.path_prefix
            );
            webdock_plugin_http::storage::bind_storage(
                "GET",
                &object,
                BTreeMap::new(),
                binding.clone(),
            )
            .map_err(|_| "Invalid storage target binding")?;
        }
    }
    Ok(())
}

impl KernelIdentity {
    fn validate(&self, policy: &TargetPolicy) -> Result<(), Error> {
        if private_text(&policy.core.certificate)?.trim() != self.certificate.trim()
            || private_text(&policy.core.private_key)?.trim() != self.private_key.trim()
            || private_text(&policy.core.authority)?.trim() != self.authority.trim()
            || private_text(&policy.runtime_identity.authority)?.trim() != self.authority.trim()
            || policy.core.server_certificate_sha256 != self.fingerprint
        {
            return Err("Unsupported managed kernel trust profile".into());
        }
        Ok(())
    }
}

async fn discover<P, F, G>(
    probe: &P,
    plugin: &str,
    generation: u64,
    mut guard: F,
) -> Result<webdock_plugin_host::RuntimeDescription, PluginError>
where
    P: webdock_plugin_host::StateProbe,
    F: FnMut() -> G,
    G: std::future::Future<Output = Result<bool, PluginError>>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    for _ in 0..12 {
        if !guard().await? {
            return Err(PluginError::denied());
        }
        let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now()) else {
            break;
        };
        let result = tokio::time::timeout(
            remaining.min(Duration::from_secs(5)),
            probe.describe(plugin, generation),
        )
        .await;
        match result {
            Ok(Ok(description)) => {
                if !guard().await? {
                    return Err(PluginError::denied());
                }
                return Ok(description);
            }
            Ok(Err(error))
                if ![ErrorCode::Unavailable, ErrorCode::DeadlineExceeded].contains(&error.code) =>
            {
                return Err(error);
            }
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err(PluginError::new(
        ErrorCode::Unavailable,
        "Runtime discovery unavailable",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn managed_policy_accepts_explicit_project_binary_bindings() {
        let value = json!({"runtimeDomain":"plugins.example.test","runtimePort":8443,"entryPoint":"websecure",
            "core":{"origin":"https://core.example.test","domain":"core.example.test","certificate":"/private/core.crt","privateKey":"/private/core.key","authority":"/private/ca.crt","serverCertificateSha256":"a".repeat(64)},
            "runtimeIdentity":{"authority":"/private/ca.crt","privateKey":"/private/core.key"},"resources":{"cpuMillicores":100,"memoryBytes":268435456},"egress":[],
            "storage":{"124":{"objects":{"origin":"https://storage.invalid","addresses":["127.0.0.1"],"ca":null,"pathPrefix":"/private/"}}}});
        let policy = decode::<TargetPolicy>(value)
            .expect("operator-approved project storage must survive native installation");
        validate_storage_bindings(&policy.storage).unwrap();
        assert_eq!(policy.storage["124"]["objects"].path_prefix, "/private/");
        let mut origin_scope = policy.storage.clone();
        origin_scope
            .get_mut("124")
            .unwrap()
            .get_mut("objects")
            .unwrap()
            .path_prefix = "/".into();
        validate_storage_bindings(&origin_scope).unwrap();
        let mut invalid = policy.storage;
        invalid
            .get_mut("124")
            .unwrap()
            .get_mut("objects")
            .unwrap()
            .origin = "http://127.0.0.1".into();
        assert!(
            validate_storage_bindings(&invalid).is_err(),
            "managed installs require HTTPS even for loopback fixtures"
        );
    }
    #[test]
    fn lifecycle_seed_plan_preserves_masters_and_never_repairs_partial_identity() {
        let empty = BTreeMap::new();
        assert_eq!(seeds_needed("install", &empty).unwrap(), (true, true));
        assert!(seeds_needed("update", &empty).is_err());
        assert!(seeds_needed("rollback", &empty).is_err());
        let mut retained = BTreeMap::from([
            ("storagePassword".into(), "a".repeat(64)),
            ("sealingKey".into(), "b".repeat(64)),
        ]);
        assert_eq!(seeds_needed("update", &retained).unwrap(), (false, true));
        assert_eq!(seeds_needed("rollback", &retained).unwrap(), (false, true));
        retained.insert("certificate".into(), "retained".into());
        assert!(seeds_needed("update", &retained).is_err());
        retained.insert("privateKey".into(), "retained".into());
        retained.insert("certificateSha256".into(), "retained".into());
        assert_eq!(seeds_needed("update", &retained).unwrap(), (false, false));
    }
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    struct Probe {
        calls: AtomicUsize,
        failures: usize,
        denied: bool,
        revoke_on_success: bool,
        allowed: Arc<AtomicBool>,
    }
    #[tonic::async_trait]
    impl webdock_plugin_host::StateProbe for Probe {
        async fn describe(
            &self,
            plugin: &str,
            generation: u64,
        ) -> Result<webdock_plugin_host::RuntimeDescription, PluginError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if self.denied {
                return Err(PluginError::denied());
            }
            if call < self.failures {
                return Err(PluginError::new(
                    ErrorCode::Unavailable,
                    "fixture route not converged",
                ));
            }
            if self.revoke_on_success {
                self.allowed.store(false, Ordering::SeqCst);
            }
            Ok(webdock_plugin_host::RuntimeDescription {
                manifest: b"owned bytes".to_vec(),
                plugin_id: plugin.into(),
                version: "0.1.0".into(),
                generation,
                protocol_version: 1,
                peer: webdock_plugin_runtime::PeerIdentity::Unix { uid: 0 },
            })
        }
    }
    #[tokio::test]
    async fn discovery_recovers_route_propagation_without_skipping_live_authority() {
        let allowed = Arc::new(AtomicBool::new(true));
        let probe = Probe {
            calls: AtomicUsize::new(0),
            failures: 2,
            denied: false,
            revoke_on_success: false,
            allowed: allowed.clone(),
        };
        let result = discover(&probe, "owned", 1, || {
            std::future::ready(Ok(allowed.load(Ordering::SeqCst)))
        })
        .await
        .unwrap();
        assert_eq!(probe.calls.load(Ordering::SeqCst), 3);
        assert_eq!(result.manifest, b"owned bytes");
    }
    #[tokio::test]
    async fn discovery_never_retries_denied_identity_or_releases_a_revoked_result() {
        for (denied, revoke_on_success) in [(true, false), (false, true)] {
            let allowed = Arc::new(AtomicBool::new(true));
            let probe = Probe {
                calls: AtomicUsize::new(0),
                failures: 0,
                denied,
                revoke_on_success,
                allowed: allowed.clone(),
            };
            let error = discover(&probe, "owned", 1, || {
                std::future::ready(Ok(allowed.load(Ordering::SeqCst)))
            })
            .await
            .err()
            .unwrap();
            assert_eq!(error.code, ErrorCode::PermissionDenied);
            assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
        }
    }
    #[test]
    fn managed_profile_refuses_mixed_issuers_and_distinct_core_leaves() {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, value: &str| {
            let path = dir.path().join(name);
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&path)
                .unwrap();
            file.write_all(value.as_bytes()).unwrap();
            path
        };
        let cert = write("core.crt", "same-core-leaf");
        let key = write("core.key", "same-core-key");
        let ca = write("ca.crt", "same-kernel-ca");
        let mut policy:TargetPolicy=decode(json!({"runtimeDomain":"plugins.example.test","runtimePort":8443,"entryPoint":"websecure",
            "core":{"origin":"https://core.example.test","domain":"core.example.test","certificate":cert,"privateKey":key,"authority":ca,"serverCertificateSha256":"a".repeat(64)},
            "runtimeIdentity":{"authority":ca,"privateKey":key},"resources":{"cpuMillicores":100,"memoryBytes":268435456},"egress":[{"cidr":"127.0.0.1/32","port":6140}]})).unwrap();
        let identity = KernelIdentity {
            certificate: "same-core-leaf".into(),
            private_key: "same-core-key".into(),
            authority: "same-kernel-ca".into(),
            fingerprint: "a".repeat(64),
        };
        identity.validate(&policy).unwrap();
        policy.runtime_identity.authority = write("foreign-ca.crt", "different-runtime-ca");
        assert!(identity.validate(&policy).is_err());
        policy.runtime_identity.authority = ca;
        policy.core.certificate = write("client.crt", "different-core-client");
        assert!(identity.validate(&policy).is_err());
    }
}
