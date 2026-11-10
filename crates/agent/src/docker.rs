use bollard::{Docker, errors::Error as DockerError, models::*, query_parameters::*};
use futures_util::TryStreamExt;
use spitzli_supervisor::*;
use std::{
    collections::{BTreeMap, HashMap},
    net::Ipv4Addr,
    os::unix::fs::FileTypeExt,
    sync::Arc,
    time::Duration,
};
mod completed;
const HASH: &str = "webdock.dev/spec-sha256";
const GENERATION: &str = "webdock.dev/generation";
const REVISION: &str = "webdock.dev/revision";
fn network_name(spec: &RuntimeSpec) -> String {
    format!("{}-net", spec.name())
}
fn volume_name(spec: &RuntimeSpec) -> String {
    format!("{}-data", spec.name())
}
fn labels(spec: &RuntimeSpec, generation: u64) -> Result<HashMap<String, String>> {
    let mut labels: HashMap<_, _> = spec.labels().into_iter().collect();
    labels.insert(HASH.into(), digest(spec)?);
    labels.insert(GENERATION.into(), generation.to_string());
    labels.insert(REVISION.into(), spec.revision.to_string());
    Ok(labels)
}
fn volume_mount(spec: &RuntimeSpec) -> Mount {
    Mount {
        target: Some("/data".into()),
        source: Some(volume_name(spec)),
        typ: Some(MountType::VOLUME),
        read_only: Some(false),
        volume_options: Some(MountVolumeOptions {
            no_copy: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    }
}
pub fn container_config(spec: &RuntimeSpec, generation: u64) -> Result<ContainerCreateBody> {
    spec.validate()?;
    if spec.replicas > 1 {
        return Err(ExecutionError::Unsupported("Docker replicas"));
    }
    Ok(ContainerCreateBody {
        image: Some(spec.image.clone()),
        cmd: Some(spec.arguments.clone()),
        user: Some("65532:65532".into()),
        labels: Some(labels(spec, generation)?),
        host_config: Some(HostConfig {
            readonly_rootfs: Some(true),
            privileged: Some(false),
            cap_drop: Some(vec!["ALL".into()]),
            security_opt: Some(vec!["no-new-privileges:true".into()]),
            memory: Some(spec.memory_bytes as i64),
            memory_swap: Some(spec.memory_bytes as i64),
            nano_cpus: Some(i64::from(spec.cpu_millicores) * 1_000_000),
            pids_limit: Some(128),
            network_mode: Some(network_name(spec)),
            tmpfs: Some(HashMap::from([(
                "/tmp".into(),
                format!(
                    "rw,noexec,nosuid,nodev,size={},mode=1777",
                    spec.ephemeral_bytes
                ),
            )])),
            mounts: spec.volume.as_ref().map(|_| vec![volume_mount(spec)]),
            restart_policy: Some(RestartPolicy {
                name: Some(RestartPolicyNameEnum::UNLESS_STOPPED),
                ..Default::default()
            }),
            log_config: Some(HostConfigLogConfig {
                typ: Some("json-file".into()),
                config: Some(HashMap::from([
                    ("max-size".into(), "1m".into()),
                    ("max-file".into(), "2".into()),
                ])),
            }),
            ..Default::default()
        }),
        ..Default::default()
    })
}
#[derive(Clone)]
pub struct DockerAdapter {
    docker: Docker,
    installation: String,
    initializer: Option<String>,
    storage: Option<(crate::storage::StorageClient, crate::storage::StorageConfig)>,
    http: reqwest::Client,
    policy: Arc<crate::policy::RuntimePolicy>,
    app_fences: Option<Arc<crate::recovery::effect_fence::AppEffectFenceRegistry>>,
}
struct AppCommandGuard {
    upstream: LeaseGuard,
    permit: Arc<crate::recovery::effect_fence::AppEffectPermit>,
}
#[async_trait::async_trait]
impl Authorizer for AppCommandGuard {
    async fn authorize(&self, command: &Command) -> Result<()> {
        self.permit.check()?;
        self.upstream.check_for(command).await?;
        self.permit.check()
    }
}
impl DockerAdapter {
    /// Deliberately ignores DOCKER_HOST and context defaults; only an explicitly supplied local socket.
    pub async fn connect(
        socket: &str,
        installation: String,
        volume_initializer_image: Option<String>,
    ) -> Result<Self> {
        if !socket.starts_with('/')
            || !std::fs::symlink_metadata(socket)
                .map_err(|_| ExecutionError::Runtime)?
                .file_type()
                .is_socket()
            || volume_initializer_image
                .as_ref()
                .is_some_and(|s| !image_digest(s))
        {
            return Err(ExecutionError::Invalid(
                "Docker socket or initializer digest",
            ));
        }
        let docker = Docker::connect_with_unix(socket, 10, bollard::API_DEFAULT_VERSION)
            .map_err(|_| ExecutionError::Runtime)?
            .negotiate_version()
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(3))
            .build()
            .map_err(|_| ExecutionError::Runtime)?;
        Ok(Self {
            docker,
            installation,
            initializer: volume_initializer_image,
            storage: None,
            http,
            policy: Arc::new(crate::policy::RuntimePolicy::default()),
            app_fences: None,
        })
    }
    fn registry_ready(&self) -> Result<bool> {
        let Some(registry) = self.policy.registry()? else {
            return Ok(false);
        };
        for grant in registry.projects.values() {
            crate::registry::RegistryCredential::load(
                grant
                    .credential_file
                    .as_ref()
                    .ok_or(ExecutionError::Ownership)?,
                grant
                    .repository
                    .split('/')
                    .next()
                    .ok_or(ExecutionError::Ownership)?,
            )?;
        }
        Ok(true)
    }
    pub fn with_policy(mut self, policy: Arc<crate::policy::RuntimePolicy>) -> Self {
        self.policy = policy;
        self
    }
    pub(crate) fn with_app_fences(
        mut self,
        fences: Arc<crate::recovery::effect_fence::AppEffectFenceRegistry>,
    ) -> Self {
        self.app_fences = Some(fences);
        self
    }
    /// Binds the local privileged helper to the explicitly connected rootful Engine node.
    pub async fn with_storage_socket(mut self, socket: std::path::PathBuf) -> Result<Self> {
        let client = crate::storage::StorageClient::new(socket);
        let config = client.information().await?;
        let info = self
            .docker
            .info()
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        if rustix::process::geteuid().as_raw() != 0
            || info.name.as_deref() != Some(config.node.as_str())
            || info
                .security_options
                .unwrap_or_default()
                .iter()
                .any(|s| s.contains("rootless") || s.contains("userns"))
        {
            return Err(ExecutionError::Unsupported("rootful storage node binding"));
        }
        let capacity = client
            .capability(std::slice::from_ref(&config.node))
            .await?;
        if !capacity.ready || capacity.capacity_bytes != config.capacity_bytes {
            return Err(ExecutionError::Runtime);
        }
        self.storage = Some((client, config));
        Ok(self)
    }
    async fn ensure_quota_volume(
        &self,
        spec: &RuntimeSpec,
        guard: &LeaseGuard,
        allow_create: bool,
        lease_until: u64,
    ) -> Result<()> {
        let (client, config) = self.storage.as_ref().ok_or(ExecutionError::Unsupported(
            "Docker volume quotas require verified helper",
        ))?;
        let name = volume_name(spec);
        let existing = match self.docker.inspect_volume(&name).await {
            Ok(volume) => {
                verify_quota_volume(&volume, spec, config)?;
                true
            }
            Err(DockerError::DockerResponseServerError {
                status_code: 404, ..
            }) => {
                if !allow_create || spec.revision != 1 {
                    return Err(ExecutionError::Ownership);
                }
                false
            }
            Err(_) => return Err(ExecutionError::Runtime),
        };
        guard.check().await?;
        let packet = crate::storage::StoragePacket::new(
            spec.resource_id.as_str(),
            spec.project_id.as_str(),
            config.cluster_id.as_str(),
            spec.revision,
            spec.volume
                .as_ref()
                .and_then(|v| v.size_bytes)
                .ok_or(ExecutionError::Invalid("quota volume"))?,
            lease_until,
        )?;
        let (actual, path) = client.ensure(&packet).await?;
        guard.check().await?;
        if actual != *config {
            return Err(ExecutionError::Ownership);
        }
        if !existing {
            let mut labels: HashMap<_, _> = spec.labels().into_iter().collect();
            labels.insert("webdock.dev/storage-pool".into(), config.pool_id.clone());
            labels.insert(
                "webdock.dev/storage-bytes".into(),
                packet.spec.volume_bytes.to_string(),
            );
            self.docker
                .create_volume(VolumeCreateRequest {
                    name: Some(name),
                    driver: Some("local".into()),
                    driver_opts: Some(HashMap::from([
                        ("type".into(), "none".into()),
                        ("o".into(), "bind".into()),
                        (
                            "device".into(),
                            path.to_str().ok_or(ExecutionError::Ownership)?.into(),
                        ),
                    ])),
                    labels: Some(labels),
                    ..Default::default()
                })
                .await
                .map_err(|_| ExecutionError::Runtime)?;
        }
        let volume = self
            .docker
            .inspect_volume(&volume_name(spec))
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        verify_quota_volume(&volume, spec, config)
    }
    fn configured_container(&self, command: &Command) -> Result<ContainerCreateBody> {
        if command
            .spec
            .hosted_access
            .as_ref()
            .is_some_and(|a| a.service_bindings.is_some())
        {
            return Err(ExecutionError::Unsupported("service binding egress"));
        }
        let spec = &command.spec;
        let generation = command.generation;
        let operation_id = &command.operation_id;
        let mut config = container_config(spec, generation)?;
        config
            .labels
            .as_mut()
            .unwrap()
            .insert("webdock.dev/operation-id".into(), operation_id.clone());
        let grants = self.policy.local_mounts(spec)?;
        if !grants.is_empty() {
            let uid = self.policy.uid(spec)?;
            config.user = Some(format!("{uid}:{uid}"));
            let mounts = config
                .host_config
                .as_mut()
                .unwrap()
                .mounts
                .get_or_insert_with(Vec::new);
            for grant in grants {
                mounts.push(Mount {
                    source: Some(grant.source.to_str().ok_or(ExecutionError::Denied)?.into()),
                    target: Some(grant.target),
                    typ: Some(MountType::BIND),
                    read_only: Some(grant.read_only),
                    ..Default::default()
                });
            }
        }
        if spec.hosted_access.is_some() && command.action == Action::Apply {
            command.verify_material()?;
            let environment = command
                .material
                .as_ref()
                .and_then(|m| m.environment.as_ref())
                .ok_or(ExecutionError::Denied)?;
            config.env = Some(
                environment
                    .values
                    .iter()
                    .map(|(key, value)| format!("{key}={value}"))
                    .collect(),
            );
            config.labels.as_mut().unwrap().insert(
                "webdock.dev/environment-source".into(),
                environment.source_sha256.clone(),
            );
        }
        if let Some(plugin) = &spec.plugin {
            config.env = Some(vec![
                format!("SPITZLI_PLUGIN_ID={}", plugin.plugin_id),
                format!("SPITZLI_PLUGIN_VERSION={}", plugin.version),
                format!("SPITZLI_PLUGIN_GENERATION={}", plugin.runtime_generation),
                format!("SPITZLI_PLUGIN_MANIFEST_SHA256={}", plugin.manifest_sha256),
            ]);
            config.labels.as_mut().unwrap().insert(
                "webdock.dev/manifest-sha256".into(),
                plugin.manifest_sha256.clone(),
            );
        }
        Ok(config)
    }
    fn validate(&self, spec: &RuntimeSpec) -> Result<()> {
        spec.validate()?;
        if spec.volume.as_ref().is_some_and(|v| v.size_bytes.is_some()) {
            if self.storage.is_none() {
                return Err(ExecutionError::Unsupported(
                    "Docker volume quotas require verified helper",
                ));
            }
            if self.policy.uid(spec)? != 65532 {
                return Err(ExecutionError::Unsupported("quota volume runtime UID"));
            }
        }
        if spec.hosting_binding.is_some() {
            return Err(ExecutionError::Unsupported(
                "retained Kubernetes binding cannot move to Docker",
            ));
        }
        if spec.installation != self.installation {
            return Err(ExecutionError::Ownership);
        }
        Ok(())
    }
    async fn owned(
        &self,
        spec: &RuntimeSpec,
        name: &str,
    ) -> Result<Option<ContainerInspectResponse>> {
        match self
            .docker
            .inspect_container(name, None::<InspectContainerOptions>)
            .await
        {
            Ok(container) => {
                let labels: BTreeMap<_, _> = container
                    .config
                    .as_ref()
                    .and_then(|c| c.labels.clone())
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
                spec.check_owner(&labels)?;
                let revision = labels
                    .get(REVISION)
                    .and_then(|v| v.parse::<u64>().ok())
                    .ok_or(ExecutionError::Ownership)?;
                if revision > spec.revision {
                    return Err(ExecutionError::Fenced);
                }
                Ok(Some(container))
            }
            Err(DockerError::DockerResponseServerError {
                status_code: 404, ..
            }) => Ok(None),
            Err(_) => Err(ExecutionError::Runtime),
        }
    }
    async fn image(&self, image: &str, project: Option<&str>, guard: &LeaseGuard) -> Result<()> {
        guard.check().await?;
        let credentials =
            if let Some(project) = project {
                if let Some(registry) = self.policy.registry()? {
                    if let Some(grant) = registry.select(project, image)? {
                        Some(
                            crate::registry::RegistryCredential::load(
                                grant.credential_file.as_ref().ok_or(
                                    ExecutionError::Unsupported("Docker registry credential file"),
                                )?,
                                image.split('/').next().ok_or(ExecutionError::Ownership)?,
                            )?
                            .docker(),
                        )
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };
        match self.docker.inspect_image(image).await {
            Ok(_) => (),
            Err(DockerError::DockerResponseServerError {
                status_code: 404, ..
            }) => {
                guard.check().await?;
                let mut stream = self.docker.create_image(
                    Some(
                        CreateImageOptionsBuilder::default()
                            .from_image(image)
                            .build(),
                    ),
                    None,
                    credentials,
                );
                loop {
                    guard.check().await?;
                    if stream
                        .try_next()
                        .await
                        .map_err(|_| ExecutionError::Runtime)?
                        .is_none()
                    {
                        break;
                    }
                    guard.check().await?;
                }
            }
            Err(_) => return Err(ExecutionError::Runtime),
        }
        let inspection = self
            .docker
            .inspect_image(image)
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        let expected = image
            .split_once('@')
            .ok_or(ExecutionError::Invalid("digest"))?
            .1;
        if !inspection
            .repo_digests
            .unwrap_or_default()
            .iter()
            .any(|s| s.split_once('@').is_some_and(|(_, d)| d == expected))
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(())
    }
    async fn ensure_network(&self, spec: &RuntimeSpec, guard: &LeaseGuard) -> Result<()> {
        let name = network_name(spec);
        match self
            .docker
            .inspect_network(&name, None::<InspectNetworkOptions>)
            .await
        {
            Ok(network) => {
                spec.check_owner(&network.labels.unwrap_or_default().into_iter().collect())?;
                if network.internal != Some(true) || network.driver.as_deref() != Some("bridge") {
                    return Err(ExecutionError::Ownership);
                }
            }
            Err(DockerError::DockerResponseServerError {
                status_code: 404, ..
            }) => {
                guard.check().await?;
                self.docker
                    .create_network(NetworkCreateRequest {
                        name,
                        driver: Some("bridge".into()),
                        internal: Some(true),
                        labels: Some(spec.labels().into_iter().collect()),
                        ..Default::default()
                    })
                    .await
                    .map_err(|_| ExecutionError::Runtime)?;
            }
            Err(_) => return Err(ExecutionError::Runtime),
        }
        Ok(())
    }
    async fn stop_owned(
        &self,
        spec: &RuntimeSpec,
        name: &str,
        generation: u64,
        guard: &LeaseGuard,
        remove: bool,
    ) -> Result<()> {
        if let Some(container) = self.owned(spec, name).await? {
            fence(&container, spec.revision, generation)?;
            let actual_id = container.id.as_deref().ok_or(ExecutionError::Ownership)?;
            if container.state.as_ref().and_then(|s| s.running) == Some(true) {
                guard.check().await?;
                self.docker
                    .stop_container(
                        actual_id,
                        Some(StopContainerOptionsBuilder::default().t(5).build()),
                    )
                    .await
                    .map_err(|_| ExecutionError::Runtime)?;
            }
            if remove {
                guard.check().await?;
                self.docker
                    .remove_container(
                        actual_id,
                        Some(
                            RemoveContainerOptionsBuilder::default()
                                .v(false)
                                .force(false)
                                .build(),
                        ),
                    )
                    .await
                    .map_err(|_| ExecutionError::Runtime)?;
            }
        }
        Ok(())
    }
    async fn lifecycle_volume(&self, spec: &RuntimeSpec) -> Result<Option<Volume>> {
        match self.docker.inspect_volume(&volume_name(spec)).await {
            Ok(volume) => {
                spec.check_owner(&volume.labels.clone().into_iter().collect())?;
                let desired = spec.volume.as_ref().ok_or(ExecutionError::Ownership)?;
                if desired.size_bytes.is_some() {
                    let (_, config) = self.storage.as_ref().ok_or(ExecutionError::Ownership)?;
                    verify_quota_volume(&volume, spec, config)?;
                } else if volume.driver != "local" || !volume.options.is_empty() {
                    return Err(ExecutionError::Ownership);
                }
                Ok(Some(volume))
            }
            Err(DockerError::DockerResponseServerError {
                status_code: 404, ..
            }) => Ok(None),
            Err(_) => Err(ExecutionError::Runtime),
        }
    }
    async fn delete_network(&self, spec: &RuntimeSpec, guard: &LeaseGuard) -> Result<()> {
        match self
            .docker
            .inspect_network(&network_name(spec), None::<InspectNetworkOptions>)
            .await
        {
            Ok(network) => {
                spec.check_owner(&network.labels.unwrap_or_default().into_iter().collect())?;
                if network.internal != Some(true)
                    || network.driver.as_deref() != Some("bridge")
                    || !network.containers.unwrap_or_default().is_empty()
                {
                    return Err(ExecutionError::Ownership);
                }
                let id = network.id.ok_or(ExecutionError::Ownership)?;
                guard.check().await?;
                self.docker
                    .remove_network(&id)
                    .await
                    .map_err(|_| ExecutionError::Runtime)?;
            }
            Err(DockerError::DockerResponseServerError {
                status_code: 404, ..
            }) => {}
            Err(_) => return Err(ExecutionError::Runtime),
        }
        Ok(())
    }
    async fn retire_or_purge(
        &self,
        command: &Command,
        guard: &LeaseGuard,
    ) -> Result<ObservedState> {
        command.validate()?;
        guard.check_for(command).await?;
        let spec = &command.spec;
        self.validate(spec)?;
        self.check_expected(command).await?;
        let volume = self.lifecycle_volume(spec).await?;
        let packet = if let Some(bytes) = spec.volume.as_ref().and_then(|v| v.size_bytes) {
            let (helper, config) = self.storage.as_ref().ok_or(ExecutionError::Ownership)?;
            if helper.information().await? != *config {
                return Err(ExecutionError::Ownership);
            }
            let packet = crate::storage::StoragePacket::new(
                spec.resource_id.as_str(),
                spec.project_id.as_str(),
                config.cluster_id.as_str(),
                spec.revision,
                bytes,
                command.lease_until,
            )?;
            guard.check().await?;
            helper.retained(&packet).await?;
            Some(packet)
        } else {
            None
        };
        if command.action == Action::Retire {
            self.stop_owned(
                spec,
                &format!("{}-init", spec.name()),
                command.generation,
                guard,
                true,
            )
            .await?;
            self.stop_owned(spec, &spec.name(), command.generation, guard, true)
                .await?;
        } else {
            // All consumers matter, including stopped containers and containers with other labels.
            if self.owned(spec, &spec.name()).await?.is_some()
                || self
                    .owned(spec, &format!("{}-init", spec.name()))
                    .await?
                    .is_some()
            {
                return Err(ExecutionError::Ownership);
            }
            let consumers = self
                .docker
                .list_containers(Some(
                    ListContainersOptionsBuilder::default()
                        .all(true)
                        .filters(&HashMap::from([(
                            "volume".to_string(),
                            vec![volume_name(spec)],
                        )]))
                        .build(),
                ))
                .await
                .map_err(|_| ExecutionError::Runtime)?;
            if !consumers.is_empty() {
                return Err(ExecutionError::Ownership);
            }
            if volume.is_some() {
                guard.check().await?;
                self.docker
                    .remove_volume(
                        &volume_name(spec),
                        Some(RemoveVolumeOptionsBuilder::default().force(false).build()),
                    )
                    .await
                    .map_err(|_| ExecutionError::Runtime)?;
            }
            if self.lifecycle_volume(spec).await?.is_some() {
                return Err(ExecutionError::Runtime);
            }
            if let Some(packet) = &packet {
                guard.check().await?;
                self.storage
                    .as_ref()
                    .ok_or(ExecutionError::Ownership)?
                    .0
                    .purge(packet)
                    .await?;
            }
        }
        self.delete_network(spec, guard).await?;
        guard.check_for(command).await?;
        if self.owned(spec, &spec.name()).await?.is_some() {
            return Err(ExecutionError::Ownership);
        }
        Ok(ObservedState {
            autoscaling: None,
            resource_id: spec.resource_id.to_string(),
            revision: spec.revision,
            phase: Phase::Absent,
            ready: true,
            observed_at: timestamp()?,
            runtime_id: None,
            conditions: vec![Condition {
                kind: "Retired".into(),
                satisfied: true,
                reason: "Owned runtime is absent".into(),
            }],
            storage: spec.volume.as_ref().map(|v| StorageObservation {
                volume_bytes: if command.action == Action::PurgeStorage {
                    Some(0)
                } else {
                    v.size_bytes
                },
                retained: command.action == Action::Retire,
            }),
        })
    }
    async fn ensure_volume(
        &self,
        spec: &RuntimeSpec,
        generation: u64,
        guard: &LeaseGuard,
        allow_create: bool,
        lease_until: u64,
    ) -> Result<()> {
        if spec.volume.is_none() {
            return Ok(());
        }
        if spec.volume.as_ref().is_some_and(|v| v.size_bytes.is_some()) {
            return self
                .ensure_quota_volume(spec, guard, allow_create, lease_until)
                .await;
        }
        let initializer = self
            .initializer
            .as_ref()
            .ok_or(ExecutionError::Unsupported("retained volume initializer"))?;
        let name = volume_name(spec);
        match self.docker.inspect_volume(&name).await {
            Ok(volume) => {
                spec.check_owner(&volume.labels.into_iter().collect())?;
                if volume.driver != "local" || !volume.options.is_empty() {
                    return Err(ExecutionError::Ownership);
                }
            }
            Err(DockerError::DockerResponseServerError {
                status_code: 404, ..
            }) => {
                if !allow_create {
                    return Err(ExecutionError::Ownership);
                }
                guard.check().await?;
                self.docker
                    .create_volume(VolumeCreateRequest {
                        name: Some(name),
                        driver: Some("local".into()),
                        labels: Some(spec.labels().into_iter().collect()),
                        ..Default::default()
                    })
                    .await
                    .map_err(|_| ExecutionError::Runtime)?;
            }
            Err(_) => return Err(ExecutionError::Runtime),
        }
        let volume = self
            .docker
            .inspect_volume(&volume_name(spec))
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        spec.check_owner(&volume.labels.into_iter().collect())?;
        if volume.driver != "local" || !volume.options.is_empty() {
            return Err(ExecutionError::Ownership);
        }
        self.image(initializer, None, guard).await?;
        let name = format!("{}-init", spec.name());
        self.stop_owned(spec, &name, generation, guard, true)
            .await?;
        let config = ContainerCreateBody {
            image: Some(initializer.clone()),
            entrypoint: Some(vec!["/bin/chown".into()]),
            cmd: Some(vec![
                format!("{}:{}", self.policy.uid(spec)?, self.policy.uid(spec)?),
                "/data".into(),
            ]),
            user: Some("0:0".into()),
            labels: Some(labels(spec, generation)?),
            host_config: Some(HostConfig {
                network_mode: Some("none".into()),
                readonly_rootfs: Some(true),
                cap_drop: Some(vec!["ALL".into()]),
                cap_add: Some(vec!["CHOWN".into()]),
                security_opt: Some(vec!["no-new-privileges:true".into()]),
                mounts: Some(vec![volume_mount(spec)]),
                memory: Some(16_777_216),
                nano_cpus: Some(100_000_000),
                pids_limit: Some(16),
                ..Default::default()
            }),
            ..Default::default()
        };
        guard.check().await?;
        self.docker
            .create_container(
                Some(CreateContainerOptionsBuilder::default().name(&name).build()),
                config,
            )
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        guard.check().await?;
        self.docker
            .start_container(&name, None::<StartContainerOptions>)
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        let outcome = tokio::time::timeout(
            Duration::from_secs(15),
            self.docker
                .wait_container(&name, None::<WaitContainerOptions>)
                .try_next(),
        )
        .await
        .map_err(|_| ExecutionError::Runtime)?
        .map_err(|_| ExecutionError::Runtime)?
        .ok_or(ExecutionError::Runtime)?;
        if outcome.status_code != 0 {
            return Err(ExecutionError::Runtime);
        }
        self.stop_owned(spec, &name, generation, guard, true).await
    }
    fn matches(&self, container: &ContainerInspectResponse, command: &Command) -> Result<bool> {
        let spec = &command.spec;
        let Some(config) = &container.config else {
            return Ok(false);
        };
        let Some(host) = &container.host_config else {
            return Ok(false);
        };
        let expected = self.configured_container(command)?;
        let expected_mounts = expected
            .host_config
            .as_ref()
            .and_then(|h| h.mounts.as_ref())
            .cloned()
            .unwrap_or_default();
        let actual_mounts = host.mounts.clone().unwrap_or_default();
        let mounts_match = actual_mounts.len() == expected_mounts.len()
            && expected_mounts.iter().all(|expected| {
                actual_mounts.iter().any(|actual| {
                    actual.typ == expected.typ
                        && actual.source == expected.source
                        && actual.target == expected.target
                        && actual.read_only.unwrap_or(false) == expected.read_only.unwrap_or(false)
                })
            });
        Ok(mounts_match
            && config.labels.as_ref().and_then(|l| l.get(HASH)) == Some(&digest(spec)?)
            && config.image.as_deref() == Some(&spec.image)
            && config.cmd.as_ref() == Some(&spec.arguments)
            && config.user == expected.user
            && host.readonly_rootfs == Some(true)
            && host.privileged != Some(true)
            && host
                .cap_drop
                .as_ref()
                .is_some_and(|c| c.iter().any(|v| v == "ALL"))
            && host.memory == Some(spec.memory_bytes as i64)
            && host.nano_cpus == Some(i64::from(spec.cpu_millicores) * 1_000_000)
            && host.network_mode.as_deref() == Some(&network_name(spec)))
    }
}
fn fence(container: &ContainerInspectResponse, revision: u64, generation: u64) -> Result<()> {
    let actual = container
        .config
        .as_ref()
        .and_then(|c| c.labels.as_ref())
        .and_then(|l| l.get(GENERATION))
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or(ExecutionError::Ownership)?;
    let actual_revision = container
        .config
        .as_ref()
        .and_then(|c| c.labels.as_ref())
        .and_then(|l| l.get(REVISION))
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or(ExecutionError::Ownership)?;
    if (actual_revision, actual) > (revision, generation) {
        return Err(ExecutionError::Fenced);
    }
    Ok(())
}
#[async_trait::async_trait]
impl RuntimeAdapter for DockerAdapter {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            retained_volumes: self.initializer.is_some() || self.storage.is_some(),
            volume_quotas: self.storage.is_some(),
            health: true,
            logs: true,
            metrics: true,
            upgrades: true,
            ..Capabilities::default()
        }
    }
    async fn target_observation(&self) -> Result<Option<TargetObservation>> {
        let info = self
            .docker
            .info()
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        let architecture = match info.architecture.as_deref() {
            Some("amd64" | "x86_64") => "amd64",
            Some("arm64" | "aarch64") => "arm64",
            _ => return Err(ExecutionError::Unsupported("node architecture")),
        };
        let cpu = info
            .ncpu
            .and_then(|n| u64::try_from(n).ok())
            .and_then(|n| n.checked_mul(1000))
            .ok_or(ExecutionError::Runtime)?;
        let memory = info
            .mem_total
            .and_then(|n| u64::try_from(n).ok())
            .ok_or(ExecutionError::Runtime)?;
        let id = info
            .name
            .filter(|n| !n.is_empty() && n.len() <= 160)
            .ok_or(ExecutionError::Runtime)?;
        let version = info
            .server_version
            .filter(|v| !v.is_empty() && v.len() <= 160)
            .ok_or(ExecutionError::Runtime)?;
        if cpu > 9_007_199_254_740_991 || memory > 9_007_199_254_740_991 {
            return Err(ExecutionError::Runtime);
        }
        let storage = if let Some((helper, config)) = &self.storage {
            let actual = helper.information().await;
            let capability = if actual.as_ref().is_ok_and(|actual| actual == config) {
                helper.capability(std::slice::from_ref(&id)).await.ok()
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
            registry_version: self.registry_ready().unwrap_or(false).then_some(1),
            service_bindings_version: None,
            public_routing_version: None,
            autoscaling_version: None,
            version,
            nodes: vec![TargetNode {
                id,
                role: "server".into(),
                architecture: architecture.into(),
                cpu_millicores: cpu,
                memory_bytes: memory,
            }],
            runtime_kind: "docker".into(),
        }))
    }
    async fn check_expected(&self, command: &Command) -> Result<()> {
        let Some(expected) = &command.expected_runtime_id else {
            return Ok(());
        };
        let Some(actual) = self.owned(&command.spec, &command.spec.name()).await? else {
            return if command.action == Action::Retire {
                Ok(())
            } else {
                Err(ExecutionError::Ownership)
            };
        };
        if actual.id.as_ref() == Some(expected) {
            return Ok(());
        }
        let labels = actual
            .config
            .and_then(|c| c.labels)
            .ok_or(ExecutionError::Ownership)?;
        if labels.get("webdock.dev/operation-id") == Some(&command.operation_id)
            && labels
                .get(GENERATION)
                .and_then(|v| v.parse::<u64>().ok())
                .is_some_and(|generation| generation <= command.generation)
            && labels.get(HASH) == Some(&digest(&command.spec)?)
        {
            return Ok(());
        }
        Err(ExecutionError::Ownership)
    }
    async fn reconcile(&self, command: &Command, guard: &LeaseGuard) -> Result<ObservedState> {
        use crate::recovery::effect_fence::{AppCommandStamp, AppEffectAdmission};
        let permit = match self
            .app_fences
            .as_ref()
            .ok_or(ExecutionError::Fenced)?
            .enter(
                command.spec.resource_id.as_str(),
                AppCommandStamp::from_command(command)?,
            )? {
            AppEffectAdmission::Execute(permit) => Arc::new(permit),
            AppEffectAdmission::Verify => return self.inspect_completed(command, guard).await,
        };
        let guarded = LeaseGuard::new(
            command.clone(),
            Arc::new(AppCommandGuard {
                upstream: guard.clone(),
                permit: permit.clone(),
            }),
        );
        let result = async {
            let guard = &guarded;
            let spec = &command.spec;
            if matches!(command.action, Action::Retire | Action::PurgeStorage) {
                return self.retire_or_purge(command, guard).await;
            }
            self.validate(spec)?;
            container_config(spec, command.generation)?;
            guard.check_for(command).await?;
            self.check_expected(command).await?;
            self.lifecycle_volume(spec).await?;
            if let Some(existing) = self.owned(spec, &spec.name()).await? {
                fence(&existing, spec.revision, command.generation)?;
            }
            if command.action == Action::Stop {
                self.stop_owned(spec, &spec.name(), command.generation, guard, false)
                    .await?;
                let mut observed = self.inspect(spec).await?;
                guard.check_for(command).await?;
                // A stop acknowledges the admitted intent revision without rewriting immutable container labels.
                if observed.phase == Phase::Stopped {
                    observed.revision = spec.revision;
                }
                return Ok(observed);
            }
            self.configured_container(command)?;
            self.image(&spec.image, Some(spec.project_id.as_str()), guard)
                .await?;
            self.ensure_network(spec, guard).await?;
            self.ensure_volume(
                spec,
                command.generation,
                guard,
                command.expected_runtime_id.is_none(),
                command.lease_until,
            )
            .await?;
            let existing = self.owned(spec, &spec.name()).await?;
            if let Some(container) = &existing {
                fence(container, spec.revision, command.generation)?;
                if !self.matches(container, command)? {
                    self.stop_owned(spec, &spec.name(), command.generation, guard, true)
                        .await?;
                }
            }
            if self.owned(spec, &spec.name()).await?.is_none() {
                guard.check().await?;
                self.docker
                    .create_container(
                        Some(
                            CreateContainerOptionsBuilder::default()
                                .name(&spec.name())
                                .build(),
                        ),
                        self.configured_container(command)?,
                    )
                    .await
                    .map_err(|_| ExecutionError::Runtime)?;
            }
            let container = self
                .owned(spec, &spec.name())
                .await?
                .ok_or(ExecutionError::Runtime)?;
            if spec.replicas > 0 && container.state.as_ref().and_then(|s| s.running) != Some(true) {
                guard.check().await?;
                self.docker
                    .start_container(&spec.name(), None::<StartContainerOptions>)
                    .await
                    .map_err(|_| ExecutionError::Runtime)?;
            }
            if spec.replicas == 0 {
                self.stop_owned(spec, &spec.name(), command.generation, guard, false)
                    .await?;
            }
            if apply_needs_health_wait(command.action, spec.replicas, spec.health_path.is_some()) {
                apply_health_convergence(|| guard.check_for(command), || self.inspect(spec)).await
            } else {
                guard.check().await?;
                self.inspect(spec).await
            }
        }
        .await;
        drop(guarded);
        if result.is_ok() {
            Arc::try_unwrap(permit)
                .map_err(|_| ExecutionError::Journal)?
                .confirmed()?;
        }
        result
    }
    async fn inspect_completed(
        &self,
        command: &Command,
        guard: &LeaseGuard,
    ) -> Result<ObservedState> {
        guard.check_for(command).await?;
        let observed = match command.action {
            Action::Retire | Action::PurgeStorage => self.inspect_retirement(command).await?,
            Action::Stop => self.inspect_stopped(command).await?,
            Action::Apply => {
                self.check_expected(command).await?;
                self.inspect(&command.spec).await?
            }
        };
        guard.check_for(command).await?;
        Ok(observed)
    }
    async fn inspect(&self, spec: &RuntimeSpec) -> Result<ObservedState> {
        self.validate(spec)?;
        let mut result = ObservedState {
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
        let Some(container) = self.owned(spec, &spec.name()).await? else {
            return Ok(result);
        };
        if let Some(volume) = &spec.volume {
            if self.lifecycle_volume(spec).await?.is_none() {
                return Err(ExecutionError::Ownership);
            }
            result.storage = Some(StorageObservation {
                volume_bytes: volume.size_bytes,
                retained: false,
            });
        }
        result.runtime_id = container.id.clone();
        result.revision = container
            .config
            .as_ref()
            .and_then(|c| c.labels.as_ref())
            .and_then(|l| l.get(REVISION))
            .and_then(|v| v.parse().ok())
            .ok_or(ExecutionError::Ownership)?;
        let running = container.state.as_ref().and_then(|s| s.running) == Some(true);
        result.phase = if running {
            Phase::Running
        } else {
            Phase::Stopped
        };
        result.conditions.push(Condition {
            kind: "Running".into(),
            satisfied: running,
            reason: if running {
                "ContainerRunning"
            } else {
                "ContainerStopped"
            }
            .into(),
        });
        if running {
            if let Some(path) = &spec.health_path {
                let ip = container
                    .network_settings
                    .and_then(|n| n.networks)
                    .and_then(|n| n.get(&network_name(spec)).cloned())
                    .and_then(|n| n.ip_address)
                    .and_then(|ip| ip.parse::<Ipv4Addr>().ok())
                    .filter(|ip| ip.is_private());
                result.ready = if let Some(ip) = ip {
                    let mut probe = self.http.get(format!("http://{ip}:{}{path}", spec.port));
                    if let Some(host) = &spec.health_host {
                        probe = probe.header(reqwest::header::HOST, host);
                    }
                    probe.send().await.is_ok_and(|r| r.status().is_success())
                } else {
                    false
                };
                result.conditions.push(Condition {
                    kind: "Ready".into(),
                    satisfied: result.ready,
                    reason: if result.ready {
                        "HealthProbePassed"
                    } else {
                        "HealthProbePending"
                    }
                    .into(),
                });
            } else {
                result.conditions.push(Condition {
                    kind: "Ready".into(),
                    satisfied: false,
                    reason: "NoHealthProbeConfigured".into(),
                });
            }
        }
        Ok(result)
    }
    async fn metrics(&self, spec: &RuntimeSpec) -> Result<ResourceMetrics> {
        self.validate(spec)?;
        if self.owned(spec, &spec.name()).await?.is_none() {
            return Err(ExecutionError::Runtime);
        }
        let name = spec.name();
        let mut stream = self.docker.stats(
            &name,
            Some(
                StatsOptionsBuilder::default()
                    .stream(false)
                    .one_shot(true)
                    .build(),
            ),
        );
        let sample = tokio::time::timeout(Duration::from_secs(10), stream.try_next())
            .await
            .map_err(|_| ExecutionError::Runtime)?
            .map_err(|_| ExecutionError::Runtime)?
            .ok_or(ExecutionError::Runtime)?;
        if sample.os_type.as_deref() != Some("linux") {
            return Err(ExecutionError::Unsupported("non-Linux metrics"));
        }
        Ok(ResourceMetrics {
            source: Some(MetricsSource::DockerEngine),
            window_nanoseconds: None,
            cpu_nanocores: None,
            observed_at: timestamp()?,
            cpu_nanoseconds: sample
                .cpu_stats
                .and_then(|s| s.cpu_usage)
                .and_then(|s| s.total_usage),
            memory_bytes: sample.memory_stats.and_then(|s| s.usage),
        })
    }
    async fn logs(&self, spec: &RuntimeSpec, tail: u32) -> Result<String> {
        self.validate(spec)?;
        if tail == 0 || tail > 1000 {
            return Err(ExecutionError::Invalid("log tail"));
        }
        if self.owned(spec, &spec.name()).await?.is_none() {
            return Ok(String::new());
        }
        let name = spec.name();
        let tail = tail.to_string();
        let mut stream = self.docker.logs(
            &name,
            Some(
                LogsOptionsBuilder::default()
                    .follow(false)
                    .stdout(true)
                    .stderr(true)
                    .tail(&tail)
                    .build(),
            ),
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = stream
            .try_next()
            .await
            .map_err(|_| ExecutionError::Runtime)?
        {
            let chunk = chunk.into_bytes();
            let remaining = crate::logs::LIMIT.saturating_sub(bytes.len());
            bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            if bytes.len() == crate::logs::LIMIT {
                break;
            }
        }
        Ok(crate::logs::bounded(&bytes))
    }
}

/// Validate an existing Engine volume before any helper allocation or attachment.
pub fn verify_quota_volume(
    volume: &Volume,
    spec: &RuntimeSpec,
    config: &crate::storage::StorageConfig,
) -> Result<()> {
    spec.validate()?;
    config.validate()?;
    spec.check_owner(&volume.labels.clone().into_iter().collect())?;
    let bytes = spec
        .volume
        .as_ref()
        .and_then(|v| v.size_bytes)
        .ok_or(ExecutionError::Invalid("quota volume"))?;
    let path = config
        .root
        .join(spec.resource_id.as_str())
        .join("mount/data");
    let expected = HashMap::from([
        ("type".to_string(), "none".to_string()),
        ("o".into(), "bind".into()),
        (
            "device".into(),
            path.to_str().ok_or(ExecutionError::Ownership)?.into(),
        ),
    ]);
    if volume.name != volume_name(spec)
        || volume.driver != "local"
        || volume.options != expected
        || volume.labels.get("webdock.dev/storage-pool") != Some(&config.pool_id)
        || volume.labels.get("webdock.dev/storage-bytes") != Some(&bytes.to_string())
    {
        return Err(ExecutionError::Ownership);
    }
    Ok(())
}

fn apply_needs_health_wait(action: Action, replicas: u16, configured_health: bool) -> bool {
    action == Action::Apply && replicas > 0 && configured_health
}
async fn apply_health_convergence<C, I, CF, IF>(
    mut check: C,
    mut inspect: I,
) -> Result<ObservedState>
where
    C: FnMut() -> CF,
    I: FnMut() -> IF,
    CF: std::future::Future<Output = Result<()>>,
    IF: std::future::Future<Output = Result<ObservedState>>,
{
    loop {
        check().await?;
        let observed = inspect().await?;
        check().await?;
        if observed.phase != Phase::Running || observed.runtime_id.is_none() {
            return Err(ExecutionError::Runtime);
        }
        if observed.ready {
            return Ok(observed);
        }
        // Existing Executor deadline/authority ticker owns cancellation; no new lease or timeout.
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}
#[cfg(test)]
mod health_convergence_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    fn state(ready: bool, phase: Phase) -> ObservedState {
        ObservedState {
            autoscaling: None,
            resource_id: "123".into(),
            revision: 1,
            phase,
            ready,
            observed_at: 1,
            runtime_id: if phase == Phase::Absent {
                None
            } else {
                Some("actual-observed-uid".into())
            },
            conditions: vec![Condition {
                kind: "Ready".into(),
                satisfied: ready,
                reason: if ready {
                    "HealthProbePassed"
                } else {
                    "HealthProbePending"
                }
                .into(),
            }],
            storage: Some(StorageObservation {
                volume_bytes: Some(67108864),
                retained: false,
            }),
        }
    }
    #[test]
    fn only_active_apply_with_health_enters_convergence() {
        assert!(apply_needs_health_wait(Action::Apply, 1, true));
        assert!(!apply_needs_health_wait(Action::Apply, 0, true));
        assert!(!apply_needs_health_wait(Action::Apply, 1, false));
        assert!(!apply_needs_health_wait(Action::Stop, 1, true));
    }
    #[tokio::test]
    async fn pending_then_ready_preserves_actual_observation() {
        let calls = AtomicUsize::new(0);
        let ready = state(true, Phase::Running);
        let result = apply_health_convergence(
            || async { Ok(()) },
            || {
                std::future::ready(Ok(if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    state(false, Phase::Running)
                } else {
                    ready.clone()
                }))
            },
        )
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(result, ready);
    }
    #[tokio::test]
    async fn guard_denial_prevents_another_inspection() {
        let checks = AtomicUsize::new(0);
        let calls = AtomicUsize::new(0);
        let result = apply_health_convergence(
            || {
                std::future::ready(if checks.fetch_add(1, Ordering::SeqCst) >= 2 {
                    Err(ExecutionError::Denied)
                } else {
                    Ok(())
                })
            },
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                std::future::ready(Ok(state(false, Phase::Running)))
            },
        )
        .await;
        assert!(matches!(result, Err(ExecutionError::Denied)));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn stopped_or_disappeared_never_completes_successfully() {
        for phase in [Phase::Stopped, Phase::Absent] {
            assert!(matches!(
                apply_health_convergence(
                    || async { Ok(()) },
                    || std::future::ready(Ok(state(false, phase)))
                )
                .await,
                Err(ExecutionError::Runtime)
            ));
        }
        assert!(matches!(
            apply_health_convergence(
                || async { Ok(()) },
                || async { Err(ExecutionError::Ownership) }
            )
            .await,
            Err(ExecutionError::Ownership)
        ));
    }
    #[tokio::test]
    async fn authority_change_during_probe_blocks_even_ready_result() {
        let revoked = AtomicBool::new(false);
        let result = apply_health_convergence(
            || {
                std::future::ready(if revoked.load(Ordering::SeqCst) {
                    Err(ExecutionError::Denied)
                } else {
                    Ok(())
                })
            },
            || {
                revoked.store(true, Ordering::SeqCst);
                std::future::ready(Ok(state(true, Phase::Running)))
            },
        )
        .await;
        assert!(matches!(result, Err(ExecutionError::Denied)));
    }
    #[tokio::test]
    async fn application_gate_closing_during_remote_guard_denies_the_provider_boundary() {
        use crate::recovery::effect_fence::{AppEffectFenceRegistry, AppFenceBinding};
        struct Held {
            entered: tokio::sync::Notify,
            release: tokio::sync::Notify,
        }
        #[async_trait::async_trait]
        impl Authorizer for Held {
            async fn authorize(&self, _: &Command) -> Result<()> {
                self.entered.notify_one();
                self.release.notified().await;
                Ok(())
            }
        }
        let root = tempfile::tempdir().unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let registry = AppEffectFenceRegistry::open(root.path().join("apps")).unwrap();
        let spec:RuntimeSpec=serde_json::from_value(serde_json::json!({"version":1,"installation":"test","projectId":"124","resourceId":"123","revision":1,"image":format!("docker.io/test/image@sha256:{}","a".repeat(64)),"arguments":[],"cpuMillicores":100,"memoryBytes":67108864,"ephemeralBytes":1048576,"replicas":1,"port":8080,"healthPath":null,"volume":null})).unwrap();
        let command = Command {
            operation_id: "128".into(),
            generation: 1,
            lease_until: timestamp().unwrap() + 300,
            authorization: "unit-only".into(),
            action: Action::Apply,
            spec,
            material: None,
            expected_runtime_id: None,
        };
        let held = Arc::new(Held {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let permit = match registry
            .enter(
                "123",
                crate::recovery::effect_fence::AppCommandStamp::from_command(&command).unwrap(),
            )
            .unwrap()
        {
            crate::recovery::effect_fence::AppEffectAdmission::Execute(p) => Arc::new(p),
            crate::recovery::effect_fence::AppEffectAdmission::Verify => {
                panic!("first command must acquire an execution permit")
            }
        };
        let guarded = LeaseGuard::new(
            command.clone(),
            Arc::new(AppCommandGuard {
                upstream: LeaseGuard::new(command.clone(), held.clone()),
                permit: permit.clone(),
            }),
        );
        let provider_calls = Arc::new(AtomicUsize::new(0));
        let calls = provider_calls.clone();
        let worker = tokio::spawn(async move {
            guarded.check_for(&command).await?;
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<_, ExecutionError>(())
        });
        held.entered.notified().await;
        let binding:AppFenceBinding=serde_json::from_value(serde_json::json!({"appID":"123","projectID":"124","customerID":"125","clusterID":"126","policyID":"127","policyRevision":1,"agentGeneration":1,"sourceRevision":1,"sourceUID":"a".repeat(64),"specSHA256":"b".repeat(64),"environmentSHA256":"c".repeat(64),"sourceOperationID":"128","sourceOperationGeneration":1,"sourceOperationStatus":"succeeded","sourceOperationFingerprint":"d".repeat(64),"operationID":"129","generation":1,"epoch":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","fenceFingerprint":"e".repeat(64)})).unwrap();
        let recovery = registry
            .close_authorized(binding, async { Ok(()) })
            .await
            .unwrap();
        held.release.notify_one();
        assert!(matches!(worker.await.unwrap(), Err(ExecutionError::Fenced)));
        assert_eq!(provider_calls.load(Ordering::SeqCst), 0);
        // Local future completion/drop alone cannot resolve accepted custody.
        drop(permit);
        assert!(recovery.drained().is_err());
    }
}
