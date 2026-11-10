pub mod autoscaling;
mod completed;
mod lifecycle;
mod metrics;
pub mod public_routing;
use k8s_openapi::api::{
    apps::v1::Deployment,
    core::v1::{Namespace, Node, Pod, Secret},
};
use kube::{
    Api, Client, Config,
    api::{ApiResource, DynamicObject, GroupVersionKind, ListParams, LogParams, PostParams},
    config::{KubeConfigOptions, Kubeconfig},
};
pub use lifecycle::verify_retirement_manifest;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use spitzli_supervisor::*;
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

fn helm_upgrade_options(action: Action) -> Vec<&'static str> {
    let mut options = vec![
        "--wait",
        "--timeout",
        "120s",
        "--history-max",
        "10",
        "--skip-crds",
        "--no-hooks",
    ];
    if action == Action::Apply {
        // Restore failed live changes before Helm next diffs against its last deployed release.
        options.push("--atomic");
    }
    options
}

pub fn helm_values(spec: &RuntimeSpec, generation: u64, stop: bool) -> Result<Value> {
    spec.validate()?;
    if spec.hosting_binding.is_some() {
        return Err(ExecutionError::Unsupported(
            "retained hosting uses its existing deployment",
        ));
    }
    if spec.volume.as_ref().is_some_and(|v| v.size_bytes.is_none()) {
        return Err(ExecutionError::Unsupported(
            "Kubernetes volume capacity must be explicit",
        ));
    }
    let limits = json!({"cpu":format!("{}m",spec.cpu_millicores),"memory":spec.memory_bytes.to_string(),"ephemeral-storage":spec.ephemeral_bytes.to_string()});
    let mut values = json!({"name":spec.name(),"labels":spec.labels(),"revision":spec.revision,"generation":generation,"specHash":digest(spec)?,"image":spec.image,"arguments":spec.arguments,"replicas":if stop{0}else{spec.replicas},"port":spec.port,"healthPath":spec.health_path,"healthHost":spec.health_host,"ephemeralBytes":spec.ephemeral_bytes,"resources":{"requests":limits,"limits":limits},"volume":{"enabled":spec.volume.is_some(),"sizeBytes":spec.volume.as_ref().and_then(|v|v.size_bytes).unwrap_or(0)},"plugin":spec.plugin,"mounts":spec.mounts.iter().map(|m|json!({"name":m.name,"secretName":crate::policy::RuntimePolicy::kubernetes_secret_name(spec,&m.name)})).collect::<Vec<_>>()});
    if !stop
        && spec.replicas > 0
        && let Some(policy) = &spec.autoscaling
    {
        values["autoscaling"] = json!(policy);
    }
    Ok(values)
}
#[derive(Clone)]
pub struct ApprovedChart {
    path: PathBuf,
    digest: String,
}
impl ApprovedChart {
    pub fn verify(path: impl AsRef<Path>, expected_sha256: &str) -> Result<Self> {
        let path = path.as_ref();
        if !path.is_absolute()
            || expected_sha256.len() != 64
            || !expected_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || std::fs::canonicalize(path).map_err(|_| ExecutionError::Invalid("chart path"))?
                != path
            || !path.join("Chart.yaml").is_file()
        {
            return Err(ExecutionError::Invalid("approved chart"));
        }
        if Self::digest(path)? != expected_sha256 {
            return Err(ExecutionError::Ownership);
        }
        Ok(Self {
            path: path.into(),
            digest: expected_sha256.into(),
        })
    }
    /// Hash reviewed chart files, including relative names. Symlinks and large trees are refused.
    pub fn digest(path: impl AsRef<Path>) -> Result<String> {
        fn visit(
            base: &Path,
            path: &Path,
            files: &mut Vec<PathBuf>,
            bytes: &mut u64,
        ) -> Result<()> {
            for entry in
                std::fs::read_dir(path).map_err(|_| ExecutionError::Invalid("chart directory"))?
            {
                let entry = entry.map_err(|_| ExecutionError::Invalid("chart entry"))?;
                let metadata = std::fs::symlink_metadata(entry.path())
                    .map_err(|_| ExecutionError::Invalid("chart metadata"))?;
                if metadata.file_type().is_symlink() {
                    return Err(ExecutionError::Invalid("chart symlink"));
                }
                if metadata.is_dir() {
                    visit(base, &entry.path(), files, bytes)?;
                } else if metadata.is_file() {
                    *bytes += metadata.len();
                    if *bytes > 1024 * 1024 || files.len() >= 128 {
                        return Err(ExecutionError::Invalid("chart limit"));
                    }
                    files.push(
                        entry
                            .path()
                            .strip_prefix(base)
                            .map_err(|_| ExecutionError::Invalid("chart path"))?
                            .into(),
                    );
                } else {
                    return Err(ExecutionError::Invalid("chart file"));
                }
            }
            Ok(())
        }
        let path = path.as_ref();
        let mut files = Vec::new();
        visit(path, path, &mut files, &mut 0)?;
        files.sort();
        let mut digest = Sha256::new();
        for relative in files {
            let name = relative
                .to_str()
                .ok_or(ExecutionError::Invalid("chart filename"))?;
            digest.update((name.len() as u64).to_be_bytes());
            digest.update(name.as_bytes());
            let bytes = std::fs::read(path.join(relative))
                .map_err(|_| ExecutionError::Invalid("chart file"))?;
            digest.update((bytes.len() as u64).to_be_bytes());
            digest.update(bytes);
        }
        Ok(format!("{:x}", digest.finalize()))
    }
}
#[derive(Clone)]
pub struct KubernetesAdapter {
    client: Client,
    installation: String,
    kubeconfig: PathBuf,
    context: String,
    helm: PathBuf,
    chart: ApprovedChart,
    policy: Arc<crate::policy::RuntimePolicy>,
    storage: Option<(crate::storage::StorageClient, crate::storage::StorageConfig)>,
    storage_node: Option<String>,
    public_routing: Option<crate::config::PublicRoutingConfig>,
}

fn local_storage_identity() -> Result<(String, String)> {
    fn read(path: &str) -> Result<String> {
        let mut value = String::new();
        std::fs::File::open(path)
            .and_then(|file| file.take(65).read_to_string(&mut value))
            .map_err(|_| ExecutionError::Ownership)?;
        Ok(value.trim().to_owned())
    }
    Ok((
        read("/etc/machine-id")?,
        read("/sys/class/dmi/id/product_uuid")?,
    ))
}

fn verify_storage_node(
    nodes: &[Node],
    continued: bool,
    helper_node: &str,
    selected: Option<&str>,
    machine_id: Option<&str>,
    local_system_uuid: Option<&str>,
) -> Result<()> {
    if continued || nodes.is_empty() || nodes.len() > 200 {
        return Err(ExecutionError::Ownership);
    }
    let Some(selected) = selected else {
        return if nodes.len() == 1 && nodes[0].metadata.name.as_deref() == Some(helper_node) {
            Ok(())
        } else {
            Err(ExecutionError::Unsupported(
                "single-node local storage binding",
            ))
        };
    };
    let machine = machine_id.ok_or(ExecutionError::Ownership)?;
    if selected != helper_node
        || machine.len() != 32
        || !machine
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || machine.bytes().all(|b| b == b'0')
    {
        return Err(ExecutionError::Ownership);
    }
    let matches = nodes
        .iter()
        .filter(|node| node.metadata.name.as_deref() == Some(selected))
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(ExecutionError::Ownership);
    }
    let local_system_uuid =
        uuid::Uuid::parse_str(local_system_uuid.ok_or(ExecutionError::Ownership)?)
            .map_err(|_| ExecutionError::Ownership)?;
    if local_system_uuid.is_nil() {
        return Err(ExecutionError::Ownership);
    }
    let mut systems = std::collections::BTreeSet::new();
    for candidate in nodes {
        let system = candidate
            .status
            .as_ref()
            .and_then(|status| status.node_info.as_ref())
            .ok_or(ExecutionError::Ownership)?;
        let system =
            uuid::Uuid::parse_str(&system.system_uuid).map_err(|_| ExecutionError::Ownership)?;
        if system.is_nil()
            || !systems.insert(system)
            || (candidate.metadata.name.as_deref() == Some(selected) && system != local_system_uuid)
        {
            return Err(ExecutionError::Ownership);
        }
    }
    let node = matches[0];
    // Existing retained PVs use this label; refuse an ambiguous or different affinity target.
    if node
        .metadata
        .labels
        .as_ref()
        .and_then(|labels| labels.get("kubernetes.io/hostname"))
        .map(String::as_str)
        != Some(selected)
        || nodes
            .iter()
            .filter(|node| {
                node.metadata
                    .labels
                    .as_ref()
                    .and_then(|labels| labels.get("kubernetes.io/hostname"))
                    .map(String::as_str)
                    == Some(selected)
            })
            .count()
            != 1
        || node
            .status
            .as_ref()
            .and_then(|status| status.node_info.as_ref())
            .map(|info| info.machine_id.as_str())
            != Some(machine)
    {
        return Err(ExecutionError::Ownership);
    }
    Ok(())
}
impl KubernetesAdapter {
    pub(crate) fn database_client(&self) -> Client {
        self.client.clone()
    }
    pub(crate) fn mail_runtime(
        &self,
        path: PathBuf,
    ) -> Result<crate::mail::kubernetes::KubernetesMailRuntime> {
        let storage = self.storage.clone().ok_or(ExecutionError::Unsupported(
            "Mail requires verified node-local storage",
        ))?;
        let runtime = crate::mail::kubernetes::KubernetesMailRuntime::open(
            self.client.clone(),
            storage,
            path,
        )?;
        if runtime.installation() != self.installation {
            return Err(ExecutionError::Ownership);
        }
        Ok(runtime)
    }
    /// Only an explicit kubeconfig and named context; never infer the workstation's default context.
    pub async fn connect(
        kubeconfig: PathBuf,
        context: String,
        installation: String,
        helm: PathBuf,
        chart: ApprovedChart,
    ) -> Result<Self> {
        if !kubeconfig.is_absolute()
            || context.is_empty()
            || context.len() > 128
            || context.starts_with('-')
            || !helm.is_absolute()
            || !helm.is_file()
        {
            return Err(ExecutionError::Invalid("explicit Kubernetes configuration"));
        }
        let kubeconfig = std::fs::canonicalize(kubeconfig)
            .map_err(|_| ExecutionError::Invalid("kubeconfig path"))?;
        let config_file = Kubeconfig::read_from(&kubeconfig)
            .map_err(|_| ExecutionError::Invalid("kubeconfig"))?;
        if config_file
            .auth_infos
            .iter()
            .filter_map(|v| v.auth_info.as_ref())
            .any(|auth| auth.exec.is_some())
        {
            return Err(ExecutionError::Unsupported(
                "Kubeconfig executable credentials",
            ));
        }
        let config = Config::from_custom_kubeconfig(
            config_file,
            &KubeConfigOptions {
                context: Some(context.clone()),
                ..Default::default()
            },
        )
        .await
        .map_err(|_| ExecutionError::Invalid("Kubernetes context"))?;
        if config.accept_invalid_certs || config.cluster_url.scheme_str() != Some("https") {
            return Err(ExecutionError::Invalid("Verified Kubernetes TLS required"));
        }
        let client = Client::try_from(config).map_err(|_| ExecutionError::Runtime)?;
        Ok(Self {
            client,
            installation,
            kubeconfig,
            context,
            helm,
            chart,
            policy: Arc::new(crate::policy::RuntimePolicy::default()),
            storage: None,
            storage_node: None,
            public_routing: None,
        })
    }
    pub fn with_policy(mut self, policy: Arc<crate::policy::RuntimePolicy>) -> Self {
        self.policy = policy;
        self
    }
    pub async fn with_storage_socket(self, socket: PathBuf) -> Result<Self> {
        self.with_storage_node(socket, None).await
    }
    /// One operator-selected local pool; this does not allocate across cluster nodes.
    pub async fn with_storage_node(mut self, socket: PathBuf, node: Option<&str>) -> Result<Self> {
        let helper = crate::storage::StorageClient::new(socket);
        let config = helper.information().await?;
        let nodes: Api<k8s_openapi::api::core::v1::Node> = Api::all(self.client.clone());
        let nodes = nodes
            .list(&ListParams::default().limit(201))
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        let identity = node.map(|_| local_storage_identity()).transpose()?;
        verify_storage_node(
            &nodes.items,
            nodes.metadata.continue_.is_some_and(|s| !s.is_empty()),
            &config.node,
            node,
            identity.as_ref().map(|value| value.0.as_str()),
            identity.as_ref().map(|value| value.1.as_str()),
        )?;
        let capacity = helper
            .capability(std::slice::from_ref(&config.node))
            .await?;
        if !capacity.ready || capacity.capacity_bytes != config.capacity_bytes {
            return Err(ExecutionError::Runtime);
        }
        self.storage = Some((helper, config));
        self.storage_node = node.map(str::to_owned);
        Ok(self)
    }
    async fn checked_registry_secret(
        &self,
        namespace: &str,
        name: &str,
        authority: &str,
    ) -> Result<()> {
        let api: Api<Secret> = Api::namespaced(self.client.clone(), namespace);
        let secret = api.get(name).await.map_err(|_| ExecutionError::Denied)?;
        if secret.type_.as_deref() != Some("kubernetes.io/dockerconfigjson") {
            return Err(ExecutionError::Ownership);
        }
        let data = secret.data.ok_or(ExecutionError::Ownership)?;
        if data.len() != 1 {
            return Err(ExecutionError::Ownership);
        }
        crate::registry::RegistryCredential::parse(
            &data
                .get(".dockerconfigjson")
                .ok_or(ExecutionError::Ownership)?
                .0,
            authority,
        )?;
        Ok(())
    }
    async fn registry_selection(
        &self,
        spec: &RuntimeSpec,
    ) -> Result<crate::registry::PullSelection> {
        let Some(registry) = self.policy.registry()? else {
            return Ok(crate::registry::PullSelection::Unconfigured);
        };
        let Some(grant) = registry.select(spec.project_id.as_str(), &spec.image)? else {
            return Ok(crate::registry::PullSelection::Anonymous);
        };
        let name = grant
            .pull_secret
            .as_ref()
            .ok_or(ExecutionError::Unsupported(
                "Kubernetes registry pull secret",
            ))?;
        let namespace = if spec.hosting_binding.is_some() {
            crate::legacy::namespace(spec)
        } else {
            spec.namespace()
        };
        self.checked_registry_secret(
            &namespace,
            name,
            spec.image
                .split('/')
                .next()
                .ok_or(ExecutionError::Ownership)?,
        )
        .await?;
        Ok(crate::registry::PullSelection::Secret(name.clone()))
    }
    async fn registry_ready(&self) -> Result<bool> {
        let Some(registry) = self.policy.registry()? else {
            return Ok(false);
        };
        for (project, grant) in &registry.projects {
            let name = grant
                .pull_secret
                .as_ref()
                .ok_or(ExecutionError::Ownership)?;
            let authority = grant
                .repository
                .split('/')
                .next()
                .ok_or(ExecutionError::Ownership)?;
            for namespace in [format!("wd-{project}"), format!("wdv-{project}")] {
                self.checked_registry_secret(&namespace, name, authority)
                    .await?;
            }
        }
        Ok(true)
    }
    async fn verify_mounts(&self, spec: &RuntimeSpec) -> Result<()> {
        for reference in &spec.mounts {
            let grant = self.policy.grant(spec, reference)?;
            match &grant.kind {
                crate::policy::GrantKind::SocketDirectory {} => {
                    return Err(ExecutionError::Unsupported(
                        "Kubernetes uses network plugin transport",
                    ));
                }
                crate::policy::GrantKind::Secret { sha256 } => {
                    let api: Api<Secret> = Api::namespaced(self.client.clone(), &spec.namespace());
                    let secret = api
                        .get(&crate::policy::RuntimePolicy::kubernetes_secret_name(
                            spec,
                            &grant.name,
                        ))
                        .await
                        .map_err(|_| ExecutionError::Denied)?;
                    spec.check_owner(&secret.metadata.labels.unwrap_or_default())?;
                    let data = secret.data.ok_or(ExecutionError::Denied)?;
                    if data.len() != 1
                        || data.get("value").is_none_or(|v| {
                            v.0.len() > 65536 || format!("{:x}", Sha256::digest(&v.0)) != *sha256
                        })
                    {
                        return Err(ExecutionError::Ownership);
                    }
                }
            }
        }
        Ok(())
    }
    fn validate(&self, spec: &RuntimeSpec) -> Result<()> {
        spec.validate()?;
        if spec.installation != self.installation {
            return Err(ExecutionError::Ownership);
        }
        Ok(())
    }
    async fn deployment(&self, spec: &RuntimeSpec) -> Result<Option<Deployment>> {
        let api: Api<Deployment> = Api::namespaced(self.client.clone(), &spec.namespace());
        let deployment = api
            .get_opt(&spec.name())
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        if let Some(d) = &deployment {
            spec.check_owner(&d.metadata.labels.clone().unwrap_or_default())?;
        }
        Ok(deployment)
    }
    async fn namespace(&self, spec: &RuntimeSpec, guard: &LeaseGuard) -> Result<()> {
        let api: Api<Namespace> = Api::all(self.client.clone());
        if let Some(existing) = api
            .get_opt(&spec.namespace())
            .await
            .map_err(|_| ExecutionError::Runtime)?
        {
            spec.check_owner(&existing.metadata.labels.unwrap_or_default())?;
            return Ok(());
        }
        let mut labels = spec.labels();
        labels.insert(
            "pod-security.kubernetes.io/enforce".into(),
            "restricted".into(),
        );
        let namespace:Namespace=serde_json::from_value(json!({"apiVersion":"v1","kind":"Namespace","metadata":{"name":spec.namespace(),"labels":labels}})).map_err(|_|ExecutionError::Invalid("namespace"))?;
        guard.check().await?;
        api.create(&PostParams::default(), &namespace)
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        Ok(())
    }
    async fn check_release_ownership(&self, spec: &RuntimeSpec, generation: u64) -> Result<()> {
        for (group, version, kind, name) in [
            ("apps", "v1", "Deployment", spec.name()),
            ("", "v1", "Service", spec.name()),
            ("", "v1", "ServiceAccount", spec.name()),
            (
                "",
                "v1",
                "PersistentVolumeClaim",
                format!("{}-data", spec.name()),
            ),
            ("networking.k8s.io", "v1", "NetworkPolicy", spec.name()),
        ] {
            let resource = ApiResource::from_gvk(&GroupVersionKind::gvk(group, version, kind));
            let api: Api<DynamicObject> =
                Api::namespaced_with(self.client.clone(), &spec.namespace(), &resource);
            if let Some(existing) = api
                .get_opt(&name)
                .await
                .map_err(|_| ExecutionError::Runtime)?
            {
                spec.check_owner(&existing.metadata.labels.unwrap_or_default())?;
                if kind == "Deployment" {
                    let annotations = existing.metadata.annotations.unwrap_or_default();
                    let actual = annotations
                        .get("webdock.dev/generation")
                        .and_then(|v| v.parse::<u64>().ok())
                        .ok_or(ExecutionError::Ownership)?;
                    let revision = annotations
                        .get("webdock.dev/revision")
                        .and_then(|v| v.parse::<u64>().ok())
                        .ok_or(ExecutionError::Ownership)?;
                    if (revision, actual) > (spec.revision, generation) {
                        return Err(ExecutionError::Fenced);
                    }
                }
            }
        }
        Ok(())
    }
    async fn upgrade(&self, command: &Command, guard: &LeaseGuard) -> Result<()> {
        let _ = ApprovedChart::verify(&self.chart.path, &self.chart.digest)?;
        let mut values = if command.action == Action::Stop {
            let manifest = self
                .helm_read(&command.spec, &["get", "manifest"], guard)
                .await?;
            verify_retirement_manifest(&command.spec, &manifest)?;
            let bytes = self
                .helm_read(&command.spec, &["get", "values", "--output", "json"], guard)
                .await?;
            let values: Value =
                serde_json::from_slice(&bytes).map_err(|_| ExecutionError::Ownership)?;
            if values["name"] != command.spec.name()
                || values["labels"]
                    != serde_json::to_value(command.spec.labels())
                        .map_err(|_| ExecutionError::Ownership)?
            {
                return Err(ExecutionError::Ownership);
            }
            values
        } else {
            let mut values = helm_values(&command.spec, command.generation, false)?;
            values["imagePullSecrets"] = match self.registry_selection(&command.spec).await? {
                crate::registry::PullSelection::Secret(name) => json!([name]),
                _ => json!([]),
            };
            values
        };
        if command.action == Action::Stop {
            // Stop retains the last installed configuration even after artifact/secret/registry grants are revoked.
            values["replicas"] = json!(0);
            values
                .as_object_mut()
                .ok_or(ExecutionError::Ownership)?
                .remove("autoscaling");
            values["revision"] = json!(command.spec.revision);
            values["generation"] = json!(command.generation);
        }
        let mut file = tempfile::NamedTempFile::new().map_err(|_| ExecutionError::Runtime)?;
        file.write_all(&serde_json::to_vec(&values).map_err(|_| ExecutionError::Runtime)?)
            .map_err(|_| ExecutionError::Runtime)?;
        file.flush().map_err(|_| ExecutionError::Runtime)?;
        guard.check().await?;
        // Fixed executable and arguments, no shell, hooks, plugin-provided chart or arbitrary YAML.
        let status = tokio::process::Command::new(&self.helm)
            .arg("upgrade")
            .arg("--install")
            .arg(command.spec.name())
            .arg(&self.chart.path)
            .arg("--namespace")
            .arg(command.spec.namespace())
            .arg("--kubeconfig")
            .arg(&self.kubeconfig)
            .arg("--kube-context")
            .arg(&self.context)
            .arg("--values")
            .arg(file.path())
            .args(helm_upgrade_options(command.action))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .status()
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        guard.check().await?;
        if !status.success() {
            return Err(ExecutionError::Runtime);
        }
        Ok(())
    }
}
#[async_trait::async_trait]
impl RuntimeAdapter for KubernetesAdapter {
    async fn check_expected(&self, command: &Command) -> Result<()> {
        let Some(expected) = &command.expected_runtime_id else {
            return Ok(());
        };
        self.validate(&command.spec)?;
        // This is an ownership precondition, not convergence against the new HPA policy.
        let (namespace, name, labels) = autoscaling::identity(&command.spec)?;
        let api: Api<Deployment> = Api::namespaced(self.client.clone(), &namespace);
        let Some(deployment) = api
            .get_opt(&name)
            .await
            .map_err(|_| ExecutionError::Runtime)?
        else {
            return if command.action == Action::Retire {
                Ok(())
            } else {
                Err(ExecutionError::Ownership)
            };
        };
        if deployment.metadata.uid.as_ref() != Some(expected)
            || labels.as_object().is_none_or(|required| {
                required.iter().any(|(key, value)| {
                    deployment
                        .metadata
                        .labels
                        .as_ref()
                        .and_then(|l| l.get(key))
                        .map(String::as_str)
                        != value.as_str()
                })
            })
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(())
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            retained_volumes: true,
            volume_quotas: false,
            health: true,
            logs: true,
            metrics: true,
            replicas: true,
            upgrades: true,
            ..Capabilities::default()
        }
    }
    async fn target_observation(&self) -> Result<Option<TargetObservation>> {
        let api: Api<Node> = Api::all(self.client.clone());
        let list = api
            .list(&ListParams::default().limit(201))
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        if list.items.is_empty()
            || list.items.len() > 200
            || list
                .metadata
                .continue_
                .is_some_and(|value| !value.is_empty())
        {
            return Err(ExecutionError::Runtime);
        }
        let storage_binding_valid = self.storage.as_ref().is_some_and(|(_, config)| {
            let identity = self
                .storage_node
                .as_ref()
                .map(|_| local_storage_identity())
                .transpose();
            identity.is_ok_and(|identity| {
                verify_storage_node(
                    &list.items,
                    false,
                    &config.node,
                    self.storage_node.as_deref(),
                    identity.as_ref().map(|value| value.0.as_str()),
                    identity.as_ref().map(|value| value.1.as_str()),
                )
                .is_ok()
            })
        });
        let mut nodes = Vec::new();
        let mut versions = std::collections::BTreeSet::new();
        for node in list.items {
            let status = node.status.ok_or(ExecutionError::Runtime)?;
            if !status
                .conditions
                .unwrap_or_default()
                .iter()
                .any(|c| c.type_ == "Ready" && c.status == "True")
            {
                return Err(ExecutionError::Runtime);
            }
            let info = status.node_info.ok_or(ExecutionError::Runtime)?;
            if !["amd64", "arm64"].contains(&info.architecture.as_str()) {
                return Err(ExecutionError::Unsupported("node architecture"));
            }
            versions.insert(info.kubelet_version);
            let allocatable = status.allocatable.ok_or(ExecutionError::Runtime)?;
            let cpu = parse_quantity(
                &allocatable.get("cpu").ok_or(ExecutionError::Runtime)?.0,
                true,
            )?;
            let memory = parse_quantity(
                &allocatable.get("memory").ok_or(ExecutionError::Runtime)?.0,
                false,
            )?;
            let id = node
                .metadata
                .name
                .filter(|n| !n.is_empty() && n.len() <= 160)
                .ok_or(ExecutionError::Runtime)?;
            let role = if node
                .metadata
                .labels
                .unwrap_or_default()
                .contains_key("node-role.kubernetes.io/control-plane")
            {
                "server"
            } else {
                "agent"
            };
            nodes.push(TargetNode {
                id,
                role: role.into(),
                architecture: info.architecture,
                cpu_millicores: cpu,
                memory_bytes: memory,
            });
        }
        let version = versions.into_iter().collect::<Vec<_>>().join(", ");
        if version.len() > 160 {
            return Err(ExecutionError::Runtime);
        }
        let storage = if let Some((helper, config)) = &self.storage {
            let actual = helper.information().await;
            let capability =
                if storage_binding_valid && actual.as_ref().is_ok_and(|actual| actual == config) {
                    helper
                        .capability(std::slice::from_ref(&config.node))
                        .await
                        .ok()
                        .filter(|v| v.ready && v.capacity_bytes == config.capacity_bytes)
                } else {
                    None
                };
            Some(StorageCapacity {
                capacity_bytes: capability
                    .as_ref()
                    .filter(|v| v.ready)
                    .map_or(0, |v| v.capacity_bytes),
                ready: capability.is_some_and(|v| v.ready),
            })
        } else {
            None
        };
        Ok(Some(TargetObservation {
            storage,
            environment: true,
            registry_version: self.registry_ready().await.unwrap_or(false).then_some(1),
            service_bindings_version: Some(1),
            public_routing_version: self.public_routing_ready().await.then_some(1),
            autoscaling_version: autoscaling::enabled().then_some(1),
            version,
            nodes,
            runtime_kind: "kubernetes".into(),
        }))
    }
    async fn reconcile(&self, command: &Command, guard: &LeaseGuard) -> Result<ObservedState> {
        if command.action == Action::Apply
            && command.spec.hosting_binding.is_none()
            && command
                .spec
                .hosted_access
                .as_ref()
                .is_some_and(|a| a.service_bindings.is_some())
        {
            return Err(ExecutionError::Unsupported("service binding application"));
        }
        let spec = &command.spec;
        self.validate(spec)?;
        if command.action == Action::Apply {
            self.policy.admit(spec)?;
        }
        let automatic =
            command.action == Action::Apply && spec.replicas > 0 && spec.autoscaling.is_some();
        if automatic && !autoscaling::enabled() {
            return Err(ExecutionError::Unsupported("qualified automatic scaling"));
        }
        if spec.hosting_binding.is_some() {
            let registry = if command.action == Action::Apply {
                self.registry_selection(spec).await?
            } else {
                crate::registry::PullSelection::Unconfigured
            };
            // Refuse unsupported routing before changing any customer workload.
            if spec
                .hosting_binding
                .as_ref()
                .is_some_and(|b| b.public_route.is_some())
            {
                public_routing::ingress(
                    spec,
                    self.public_routing
                        .as_ref()
                        .ok_or(ExecutionError::Unsupported("qualified public routing"))?,
                )?;
                if command.action == Action::Apply && !self.public_routing_ready().await {
                    return Err(ExecutionError::Unsupported(
                        "ready public ingress and ACME issuer",
                    ));
                }
            }
            let mut observed = crate::legacy::reconcile_with_registry(
                self.client.clone(),
                command,
                guard,
                self.storage.as_ref(),
                registry,
            )
            .await?;
            self.reconcile_public_route(command, guard, &mut observed)
                .await?;
            return Ok(observed);
        }
        if command.action == Action::Retire {
            return self.retire_release(command, guard).await;
        }
        if command.action == Action::PurgeStorage {
            return self.purge_release_storage(command, guard).await;
        }
        helm_values(spec, command.generation, command.action == Action::Stop)?;
        guard.check_for(command).await?;
        self.check_expected(command).await?;
        if command.action == Action::Stop && self.deployment(spec).await?.is_none() {
            return self.inspect(spec).await;
        }
        self.namespace(spec, guard).await?;
        self.check_release_ownership(spec, command.generation)
            .await?;
        if !automatic {
            autoscaling::remove(self.client.clone(), command, guard).await?;
        }
        // Helm owns specification; the Deployment controller owns Pods and restart behavior.
        guard.check().await?;
        if command.action == Action::Apply {
            self.verify_mounts(spec).await?;
        }
        self.upgrade(command, guard).await?;
        if automatic {
            let deployment = self
                .deployment(spec)
                .await?
                .ok_or(ExecutionError::Ownership)?;
            let uid = deployment
                .metadata
                .uid
                .as_deref()
                .ok_or(ExecutionError::Ownership)?;
            autoscaling::apply(self.client.clone(), command, guard, uid).await?;
        }
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            guard.check_for(command).await?;
            let observed = self.inspect(spec).await?;
            if ((command.action == Action::Stop || spec.replicas == 0)
                && observed.phase == Phase::Stopped)
                || (command.action == Action::Apply && spec.replicas > 0 && observed.ready)
            {
                return Ok(observed);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(ExecutionError::Runtime);
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }
    async fn inspect_completed(
        &self,
        command: &Command,
        guard: &LeaseGuard,
    ) -> Result<ObservedState> {
        command.validate()?;
        self.validate(&command.spec)?;
        guard.check_for(command).await?;
        let mut observed = match command.action {
            Action::Retire | Action::PurgeStorage if command.spec.hosting_binding.is_some() => {
                crate::legacy::inspect_completed(
                    self.client.clone(),
                    command,
                    guard,
                    self.storage.as_ref(),
                )
                .await?
            }
            Action::Retire | Action::PurgeStorage => {
                self.inspect_retirement(command, guard).await?
            }
            Action::Apply | Action::Stop => {
                self.check_expected(command).await?;
                self.inspect(&command.spec).await?
            }
        };
        self.observe_public_https(command, guard, &mut observed, false)
            .await?;
        guard.check_for(command).await?;
        Ok(observed)
    }
    async fn inspect(&self, spec: &RuntimeSpec) -> Result<ObservedState> {
        self.validate(spec)?;
        if spec.hosting_binding.is_some() {
            return crate::legacy::inspect(self.client.clone(), spec).await;
        }
        let mut observed = ObservedState {
            autoscaling: None,
            resource_id: spec.resource_id.to_string(),
            revision: spec.revision,
            phase: Phase::Absent,
            ready: false,
            observed_at: timestamp()?,
            runtime_id: None,
            conditions: vec![],
            storage: None,
        };
        let Some(deployment) = self.deployment(spec).await? else {
            return Ok(observed);
        };
        observed.runtime_id = deployment.metadata.uid;
        observed.revision = deployment
            .metadata
            .annotations
            .as_ref()
            .and_then(|a| a.get("webdock.dev/revision"))
            .and_then(|v| v.parse().ok())
            .ok_or(ExecutionError::Ownership)?;
        let desired = deployment
            .spec
            .as_ref()
            .and_then(|s| s.replicas)
            .unwrap_or(1);
        let status = deployment.status.unwrap_or_default();
        let observed_generation =
            status.observed_generation.unwrap_or(0) >= deployment.metadata.generation.unwrap_or(1);
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &spec.namespace());
        let selector = spec
            .labels()
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(",");
        let pods = pods
            .list(&ListParams::default().labels(&selector).limit(201))
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        let complete = pods.metadata.continue_.is_none_or(|v| v.is_empty());
        let stopped = desired == 0
            && status.replicas.unwrap_or(0) == 0
            && observed_generation
            && complete
            && pods.items.is_empty();
        observed.ready = desired > 0
            && observed_generation
            && status.updated_replicas == Some(desired)
            && status.available_replicas == Some(desired)
            && complete
            && pods.items.len() == desired as usize
            && pods.items.iter().all(|p| {
                p.metadata.deletion_timestamp.is_none()
                    && p.status
                        .as_ref()
                        .and_then(|s| s.conditions.as_ref())
                        .is_some_and(|c| c.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
            });
        observed.phase = if stopped {
            Phase::Stopped
        } else if observed.ready {
            Phase::Running
        } else {
            Phase::Starting
        };
        observed.conditions.push(Condition {
            kind: "Ready".into(),
            satisfied: observed.ready,
            reason: if stopped {
                "DeploymentSuspended"
            } else if observed.ready {
                "DeploymentAvailable"
            } else {
                "DeploymentProgressing"
            }
            .into(),
        });
        autoscaling::observe(self.client.clone(), spec, &mut observed).await?;
        Ok(observed)
    }
    async fn metrics(&self, spec: &RuntimeSpec) -> Result<ResourceMetrics> {
        self.resource_metrics(spec).await
    }
    async fn logs(&self, spec: &RuntimeSpec, tail: u32) -> Result<String> {
        self.validate(spec)?;
        if spec.hosting_binding.is_some() {
            return crate::legacy::logs(self.client.clone(), spec, tail).await;
        }
        if tail == 0 || tail > 1000 {
            return Err(ExecutionError::Invalid("log tail"));
        }
        let api: Api<Pod> = Api::namespaced(self.client.clone(), &spec.namespace());
        let selector = spec
            .labels()
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(",");
        let pods = api
            .list(&ListParams::default().labels(&selector).limit(20))
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        let Some(pod) = pods.items.first() else {
            return Ok(String::new());
        };
        spec.check_owner(&pod.metadata.labels.clone().unwrap_or_default())?;
        let logs = tokio::time::timeout(
            Duration::from_secs(10),
            api.logs(
                pod.metadata
                    .name
                    .as_deref()
                    .ok_or(ExecutionError::Ownership)?,
                &LogParams {
                    container: Some("runtime".into()),
                    tail_lines: Some(i64::from(tail)),
                    limit_bytes: Some(crate::logs::LIMIT as i64),
                    ..Default::default()
                },
            ),
        )
        .await
        .map_err(|_| ExecutionError::Runtime)?
        .map_err(|_| ExecutionError::Runtime)?;
        Ok(crate::logs::bounded(logs.as_bytes()))
    }
}

/// Exact bounded quantity vocabulary retained from the existing node observer.
pub fn parse_quantity(value: &str, cpu: bool) -> Result<u64> {
    parse_quantity_scaled(value, if cpu { 1000 } else { 1 })
}
fn parse_quantity_scaled(value: &str, scale: u64) -> Result<u64> {
    use rust_decimal::{Decimal, prelude::ToPrimitive};
    use std::str::FromStr;
    let split = value
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(value.len());
    let mut number = Decimal::from_str(&value[..split])
        .map_err(|_| ExecutionError::Invalid("Kubernetes quantity"))?;
    let suffix = &value[split..];
    let factor = if (suffix.starts_with('e') || suffix.starts_with('E')) && suffix.len() > 1 {
        number = Decimal::from_scientific(value)
            .map_err(|_| ExecutionError::Invalid("Kubernetes quantity exponent"))?;
        Decimal::ONE
    } else {
        match suffix {
            "" => Decimal::ONE,
            "m" => Decimal::new(1, 3),
            "u" => Decimal::new(1, 6),
            "n" => Decimal::new(1, 9),
            "Ki" => Decimal::from(1024u64),
            "Mi" => Decimal::from(1024u64.pow(2)),
            "Gi" => Decimal::from(1024u64.pow(3)),
            "Ti" => Decimal::from(1024u64.pow(4)),
            "Pi" => Decimal::from(1024u64.pow(5)),
            "Ei" => Decimal::from(1024u64.pow(6)),
            "k" | "K" => Decimal::from(1000u64),
            "M" => Decimal::from(1000u64.pow(2)),
            "G" => Decimal::from(1000u64.pow(3)),
            "T" => Decimal::from(1000u64.pow(4)),
            "P" => Decimal::from(1000u64.pow(5)),
            "E" => Decimal::from(1000u64.pow(6)),
            _ => return Err(ExecutionError::Invalid("Kubernetes quantity suffix")),
        }
    };
    let result = number
        .checked_mul(factor)
        .and_then(|v| v.checked_mul(Decimal::from(scale)))
        .ok_or(ExecutionError::Invalid("Kubernetes quantity overflow"))?;
    if !result.fract().is_zero()
        || result.is_sign_negative()
        || result > Decimal::from(9_007_199_254_740_991u64)
    {
        return Err(ExecutionError::Invalid("Kubernetes quantity range"));
    }
    result
        .to_u64()
        .ok_or(ExecutionError::Invalid("Kubernetes quantity range"))
}

#[cfg(test)]
mod helm_rollback_tests {
    use super::*;

    #[test]
    fn real_helm_failed_port_upgrade_restores_previous_release_without_duplicate_ports() {
        use spitzli_core::id::RecordId;
        let root = tempfile::tempdir().unwrap();
        let mut spec = RuntimeSpec {
            autoscaling: None,
            version: 1,
            installation: "owned".into(),
            project_id: RecordId::parse("1").unwrap(),
            resource_id: RecordId::parse("1").unwrap(),
            revision: 1,
            image: format!("docker.io/library/busybox@sha256:{}", "a".repeat(64)),
            arguments: vec!["httpd".into(), "-f".into(), "-p".into(), "8080".into()],
            cpu_millicores: 100,
            memory_bytes: 32 * 1024 * 1024,
            ephemeral_bytes: 16 * 1024 * 1024,
            replicas: 1,
            port: 8080,
            health_path: Some("/".into()),
            health_host: None,
            volume: Some(RetainedVolume {
                mount_path: "/data".into(),
                size_bytes: Some(64 * 1024 * 1024),
            }),
            plugin: None,
            mounts: Vec::new(),
            hosting_binding: None,
            hosted_access: None,
        };
        let mut files = Vec::new();
        for (revision, port) in [(1, 8080), (2, 8081), (3, 8080)] {
            spec.revision = revision;
            spec.port = port;
            spec.arguments[3] = port.to_string();
            spec.image = format!(
                "docker.io/library/busybox@sha256:{}",
                if revision == 2 { "b" } else { "a" }.repeat(64)
            );
            let mut values = helm_values(&spec, revision, false).unwrap();
            values["imagePullSecrets"] = json!([]);
            let path = root.path().join(format!("{revision}.json"));
            std::fs::write(&path, serde_json::to_vec(&values).unwrap()).unwrap();
            files.push(path);
        }
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let output = std::process::Command::new("python3")
            .arg(repo.join("tools/tests/helm-port-rollback.py"))
            .arg(repo.join("deploy/helm/webdock-plugin"))
            .args(files)
            .arg(serde_json::to_string(&helm_upgrade_options(Action::Apply)).unwrap())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn failed_stop_cannot_use_atomic_rollback_to_restore_running_replicas() {
        assert!(!helm_upgrade_options(Action::Stop).contains(&"--atomic"));
    }
}

#[cfg(test)]
mod storage_binding_tests {
    use super::*;

    fn node(name: &str, machine: Option<&str>) -> Node {
        let mut value = json!({"metadata":{"name":name,"labels":{"kubernetes.io/hostname":name}}});
        if let Some(machine) = machine {
            value["status"] = json!({"nodeInfo":{"architecture":"amd64","bootID":"boot","containerRuntimeVersion":"containerd","kernelVersion":"linux","kubeProxyVersion":"","kubeletVersion":"v1.32","machineID":machine,"operatingSystem":"linux","osImage":"linux","systemUUID":match name { "storage" => SYSTEM, "third" => THIRD, _ => FIRST }}});
        }
        serde_json::from_value(value).unwrap()
    }
    const MACHINE: &str = "11111111111111111111111111111111";
    const SYSTEM: &str = "123e4567-e89b-42d3-a456-426614174000";
    const FIRST: &str = "123e4567-e89b-42d3-a456-426614174001";
    const THIRD: &str = "123e4567-e89b-42d3-a456-426614174002";

    #[test]
    fn old_config_still_requires_exactly_one_helper_named_node() {
        let one = [node("storage", None)];
        assert!(verify_storage_node(&one, false, "storage", None, None, None).is_ok());
        assert!(verify_storage_node(&one, false, "foreign", None, None, None).is_err());
        assert!(verify_storage_node(&[], false, "storage", None, None, None).is_err());
        assert!(verify_storage_node(&one, true, "storage", None, None, None).is_err());
        let many = [node("storage", None), node("other", None)];
        assert!(verify_storage_node(&many, false, "storage", None, None, None).is_err());
    }

    #[test]
    fn cloned_machine_ids_cannot_authorize_duplicated_system_uuid() {
        let mut nodes = [
            node("first", Some(MACHINE)),
            node("storage", Some(MACHINE)),
            node("third", Some(MACHINE)),
        ];
        assert!(
            verify_storage_node(
                &nodes,
                false,
                "storage",
                Some("storage"),
                Some(MACHINE),
                Some(SYSTEM)
            )
            .is_ok()
        );
        for system in [
            None,
            Some(""),
            Some("invalid"),
            Some("00000000-0000-0000-0000-000000000000"),
            Some(FIRST),
        ] {
            assert!(
                verify_storage_node(
                    &nodes,
                    false,
                    "storage",
                    Some("storage"),
                    Some(MACHINE),
                    system
                )
                .is_err()
            );
        }
        // The cloned machine ID matches every host; a different physical host must still fail.
        nodes[1]
            .status
            .as_mut()
            .unwrap()
            .node_info
            .as_mut()
            .unwrap()
            .system_uuid = "123e4567-e89b-42d3-a456-426614174003".into();
        assert!(
            verify_storage_node(
                &nodes,
                false,
                "storage",
                Some("storage"),
                Some(MACHINE),
                Some(SYSTEM)
            )
            .is_err()
        );
        nodes[1] = node("storage", Some(MACHINE));
        nodes[0]
            .status
            .as_mut()
            .unwrap()
            .node_info
            .as_mut()
            .unwrap()
            .system_uuid = "123e4567-e89b-42d3-a456-426614174000".into();
        nodes[1]
            .status
            .as_mut()
            .unwrap()
            .node_info
            .as_mut()
            .unwrap()
            .system_uuid = "123e4567-e89b-42d3-a456-426614174000".into();
        assert!(
            verify_storage_node(
                &nodes,
                false,
                "storage",
                Some("storage"),
                Some(MACHINE),
                Some(SYSTEM)
            )
            .is_err()
        );
        nodes[0] = node("first", Some(MACHINE));
        nodes[2]
            .status
            .as_mut()
            .unwrap()
            .node_info
            .as_mut()
            .unwrap()
            .system_uuid
            .clear();
        assert!(
            verify_storage_node(
                &nodes,
                false,
                "storage",
                Some("storage"),
                Some(MACHINE),
                Some(SYSTEM)
            )
            .is_err()
        );
    }

    #[test]
    fn selected_local_storage_accepts_only_matching_helper_node_and_local_machine() {
        let nodes = [
            node("first", Some("other")),
            node("storage", Some(MACHINE)),
            node("third", Some("other")),
        ];
        assert!(
            verify_storage_node(
                &nodes,
                false,
                "storage",
                Some("storage"),
                Some(MACHINE),
                Some(SYSTEM)
            )
            .is_ok()
        );
        for (helper, selected, machine) in [
            ("first", "storage", Some(MACHINE)),
            ("storage", "missing", Some(MACHINE)),
            ("storage", "storage", None),
            ("storage", "storage", Some("")),
            (
                "storage",
                "storage",
                Some("00000000000000000000000000000000"),
            ),
            (
                "storage",
                "storage",
                Some("22222222222222222222222222222222"),
            ),
        ] {
            assert!(
                verify_storage_node(&nodes, false, helper, Some(selected), machine, Some(SYSTEM))
                    .is_err()
            );
        }
        assert!(
            verify_storage_node(
                &nodes,
                true,
                "storage",
                Some("storage"),
                Some(MACHINE),
                Some(SYSTEM)
            )
            .is_err()
        );
        let mut invalid = nodes.to_vec();
        invalid[1].status = None;
        assert!(
            verify_storage_node(
                &invalid,
                false,
                "storage",
                Some("storage"),
                Some(MACHINE),
                Some(SYSTEM)
            )
            .is_err()
        );
        invalid[1] = node("storage", Some(MACHINE));
        invalid[1]
            .metadata
            .labels
            .as_mut()
            .unwrap()
            .insert("kubernetes.io/hostname".into(), "linux-hostname".into());
        assert!(
            verify_storage_node(
                &invalid,
                false,
                "storage",
                Some("storage"),
                Some(MACHINE),
                Some(SYSTEM)
            )
            .is_err()
        );
        let mut ambiguous_affinity = nodes.to_vec();
        ambiguous_affinity[0]
            .metadata
            .labels
            .as_mut()
            .unwrap()
            .insert("kubernetes.io/hostname".into(), "storage".into());
        assert!(
            verify_storage_node(
                &ambiguous_affinity,
                false,
                "storage",
                Some("storage"),
                Some(MACHINE),
                Some(SYSTEM)
            )
            .is_err()
        );
        let duplicate = [
            node("storage", Some(MACHINE)),
            node("storage", Some(MACHINE)),
        ];
        assert!(
            verify_storage_node(
                &duplicate,
                false,
                "storage",
                Some("storage"),
                Some(MACHINE),
                Some(SYSTEM)
            )
            .is_err()
        );
        let too_many = vec![node("storage", Some(MACHINE)); 201];
        assert!(
            verify_storage_node(
                &too_many,
                false,
                "storage",
                Some("storage"),
                Some(MACHINE),
                Some(SYSTEM)
            )
            .is_err()
        );
    }
}
