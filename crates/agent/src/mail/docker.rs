//! Explicit local, isolated incoming Mail runtime. No host ports or Docker defaults.
#[path = "bootstrap.rs"]
mod bootstrap;
#[path = "configuration_docker.rs"]
mod configuration;
#[path = "initializer.rs"]
mod initializer;
use super::{MailAuthority, MailCapabilities, MailRuntime, client, tls};
use bollard::{Docker, errors::Error as DockerError, models::*, query_parameters::*};
use futures_util::TryStreamExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::HashMap, net::Ipv4Addr, os::unix::fs::FileTypeExt, time::Duration};
use webdock_agent_protocol::mail::*;
use webdock_supervisor::{ExecutionError, Result};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DockerMailConfig {
    pub image: String,
    pub installation: String,
}
impl DockerMailConfig {
    pub fn validate(&self) -> Result<()> {
        if self.installation.is_empty()
            || self.installation.len() > 24
            || !self
                .installation
                .bytes()
                .all(|v| v.is_ascii_lowercase() || v.is_ascii_digit() || v == b'-')
            || !webdock_supervisor::image_digest(&self.image)
        {
            return Err(ExecutionError::Invalid("Mail Docker configuration"));
        }
        Ok(())
    }
}
pub struct DockerMailRuntime {
    docker: Docker,
    image: String,
    installation: String,
}
fn missing(error: &DockerError) -> bool {
    matches!(
        error,
        DockerError::DockerResponseServerError {
            status_code: 404,
            ..
        }
    )
}
fn digest(value: &impl serde::Serialize) -> Result<String> {
    Ok(Sha256::digest(
        serde_json::to_vec(value).map_err(|_| ExecutionError::Invalid("Mail digest"))?,
    )
    .iter()
    .map(|v| format!("{v:02x}"))
    .collect())
}
impl DockerMailRuntime {
    pub async fn connect(socket: &str, image: String, installation: String) -> Result<Self> {
        DockerMailConfig {
            image: image.clone(),
            installation: installation.clone(),
        }
        .validate()?;
        if !socket.starts_with('/')
            || !std::fs::symlink_metadata(socket).is_ok_and(|m| m.file_type().is_socket())
        {
            return Err(ExecutionError::Invalid("Mail Docker socket"));
        }
        let docker = Docker::connect_with_unix(socket, 10, bollard::API_DEFAULT_VERSION)
            .map_err(|_| ExecutionError::Runtime)?
            .negotiate_version()
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        let inspected = docker
            .inspect_image(&image)
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        let config = inspected.config.ok_or(ExecutionError::Ownership)?;
        if config.user.as_deref() != Some("65532:65532")
            || config.cmd != Some(vec!["--config".into(), "/data/config.json".into()])
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(Self {
            docker,
            image,
            installation,
        })
    }
    fn name(&self, claim: &MailClaim) -> String {
        format!("wd-mail-{}-{}", self.installation, claim.spec.customer_id)
    }
    fn labels(&self, claim: &MailClaim) -> Result<HashMap<String, String>> {
        let credentials = match &claim.material {
            MailMaterial::Credentials {
                username, password, ..
            } => digest(&(username, password))?,
            MailMaterial::Unallocated {} => String::new(),
        };
        Ok(HashMap::from([
            (
                "webdock.mail.installation".into(),
                self.installation.clone(),
            ),
            (
                "webdock.mail.customer".into(),
                claim.spec.customer_id.clone(),
            ),
            ("webdock.mail.cluster".into(), claim.spec.cluster_id.clone()),
            (
                "webdock.mail.generation".into(),
                claim.spec.agent_generation.to_string(),
            ),
            (
                "webdock.mail.profile".into(),
                digest(&(
                    &claim.spec.image,
                    &claim.spec.hostname,
                    &claim.spec.resources,
                    credentials,
                ))?,
            ),
        ]))
    }
    fn owner(&self, labels: &HashMap<String, String>, claim: &MailClaim) -> Result<()> {
        let generation = labels
            .get("webdock.mail.generation")
            .ok_or(ExecutionError::Ownership)?;
        let observed = generation
            .parse::<i32>()
            .map_err(|_| ExecutionError::Ownership)?;
        if observed < 1
            || observed > claim.spec.agent_generation
            || observed.to_string() != *generation
            || self
                .labels(claim)?
                .iter()
                .filter(|(key, _)| key.as_str() != "webdock.mail.generation")
                .any(|(key, value)| labels.get(key) != Some(value))
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(())
    }
    async fn container(&self, claim: &MailClaim) -> Result<Option<ContainerInspectResponse>> {
        let resource = match self
            .docker
            .inspect_container(&self.name(claim), None::<InspectContainerOptions>)
            .await
        {
            Ok(v) => v,
            Err(e) if missing(&e) => return Ok(None),
            Err(_) => return Err(ExecutionError::Runtime),
        };
        self.validate_container(&resource, claim)?;
        Ok(Some(resource))
    }
    fn validate_container(
        &self,
        resource: &ContainerInspectResponse,
        claim: &MailClaim,
    ) -> Result<()> {
        let config = resource.config.as_ref().ok_or(ExecutionError::Ownership)?;
        self.owner(
            config.labels.as_ref().ok_or(ExecutionError::Ownership)?,
            claim,
        )?;
        let host = resource
            .host_config
            .as_ref()
            .ok_or(ExecutionError::Ownership)?;
        let expected = self.config(claim, self.recovery(resource))?;
        self.host_matches(
            host,
            expected
                .host_config
                .as_ref()
                .ok_or(ExecutionError::Ownership)?,
        )?;
        if config.image.as_ref() != Some(&self.image)
            || config.user != expected.user
            || config.cmd != expected.cmd
            || config.entrypoint != expected.entrypoint
            || config.env != expected.env
        {
            return Err(ExecutionError::Ownership);
        }
        let networks = resource
            .network_settings
            .as_ref()
            .and_then(|n| n.networks.as_ref())
            .ok_or(ExecutionError::Ownership)?;
        if networks.len() != 1 || !networks.contains_key(&self.name(claim)) {
            return Err(ExecutionError::Ownership);
        }
        let mounts = resource.mounts.as_ref().ok_or(ExecutionError::Ownership)?;
        if mounts.len() != 1
            || mounts[0].name.as_deref() != Some(&format!("{}-data", self.name(claim)))
            || mounts[0].destination.as_deref() != Some("/data")
            || mounts[0].typ != Some("volume".into())
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(())
    }
    fn host_matches(&self, got: &HostConfig, want: &HostConfig) -> Result<()> {
        if got.readonly_rootfs != want.readonly_rootfs
            || got.privileged.unwrap_or(false)
            || got.cap_drop != want.cap_drop
            || got.cap_add.as_deref().unwrap_or_default()
                != want.cap_add.as_deref().unwrap_or_default()
            || got.security_opt != want.security_opt
            || got.network_mode != want.network_mode
            || got.memory != want.memory
            || got.memory_swap != want.memory_swap
            || got.nano_cpus != want.nano_cpus
            || got.pids_limit != want.pids_limit
            || got.tmpfs != want.tmpfs
            || got.port_bindings.as_ref().is_some_and(|v| !v.is_empty())
            || got.binds.as_ref().is_some_and(|v| !v.is_empty())
            || got.devices.as_ref().is_some_and(|v| !v.is_empty())
            || got.pid_mode.as_deref().is_some_and(|v| !v.is_empty())
            || got.mounts != want.mounts
            || got.restart_policy.as_ref().and_then(|p| p.name)
                != want.restart_policy.as_ref().and_then(|p| p.name)
            || got
                .restart_policy
                .as_ref()
                .and_then(|p| p.maximum_retry_count)
                .unwrap_or(0)
                != 0
            || got.log_config != want.log_config
            || got.publish_all_ports.unwrap_or(false)
            || got.extra_hosts.as_ref().is_some_and(|v| !v.is_empty())
            || got.volumes_from.as_ref().is_some_and(|v| !v.is_empty())
            || got.device_requests.as_ref().is_some_and(|v| !v.is_empty())
            || got.sysctls.as_ref().is_some_and(|v| !v.is_empty())
            || got.uts_mode.as_deref().is_some_and(|v| !v.is_empty())
            || got
                .ipc_mode
                .as_deref()
                .is_some_and(|v| !v.is_empty() && v != "private")
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(())
    }
    fn recovery(&self, resource: &ContainerInspectResponse) -> bool {
        resource
            .config
            .as_ref()
            .and_then(|c| c.env.as_ref())
            .is_some_and(|env| {
                env.iter()
                    .any(|v| v.starts_with("STALWART_RECOVERY_ADMIN="))
            })
    }
    fn config(&self, claim: &MailClaim, recovery: bool) -> Result<ContainerCreateBody> {
        let mut env = vec![
            format!("STALWART_PUBLIC_URL=https://{}", claim.spec.hostname),
            "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into(),
            "STALWART_HEALTHCHECK_URL=http://127.0.0.1:8080/healthz/live".into(),
        ];
        if recovery {
            let MailMaterial::Credentials {
                bootstrap_password: Some(password),
                ..
            } = &claim.material
            else {
                return Err(ExecutionError::Ownership);
            };
            env.push("STALWART_RECOVERY_MODE=1".into());
            env.push(format!("STALWART_RECOVERY_ADMIN=admin:{password}"));
        }
        Ok(ContainerCreateBody {
            image: Some(self.image.clone()),
            user: Some("65532:65532".into()),
            entrypoint: Some(vec!["/usr/local/bin/stalwart".into()]),
            cmd: Some(vec!["--config".into(), "/data/config.json".into()]),
            env: Some(env),
            labels: Some(self.labels(claim)?),
            host_config: Some(HostConfig {
                readonly_rootfs: Some(true),
                privileged: Some(false),
                cap_drop: Some(vec!["ALL".into()]),
                security_opt: Some(vec!["no-new-privileges:true".into()]),
                memory: Some(1_073_741_824),
                memory_swap: Some(1_073_741_824),
                nano_cpus: Some(500_000_000),
                pids_limit: Some(128),
                network_mode: Some(self.name(claim)),
                tmpfs: Some(HashMap::from([
                    (
                        "/tmp".into(),
                        "rw,noexec,nosuid,nodev,size=268435456,mode=1777".into(),
                    ),
                    (
                        "/etc/stalwart".into(),
                        "rw,noexec,nosuid,nodev,size=1048576,mode=0700,uid=65532,gid=65532".into(),
                    ),
                    (
                        "/var/lib/stalwart".into(),
                        "rw,noexec,nosuid,nodev,size=1048576,mode=0700,uid=65532,gid=65532".into(),
                    ),
                ])),
                mounts: Some(vec![self.mount(claim)]),
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
    fn mount(&self, claim: &MailClaim) -> Mount {
        Mount {
            target: Some("/data".into()),
            source: Some(format!("{}-data", self.name(claim))),
            typ: Some(MountType::VOLUME),
            read_only: None,
            volume_options: Some(MountVolumeOptions {
                no_copy: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        }
    }
    async fn infrastructure(&self, claim: &MailClaim, authority: &dyn MailAuthority) -> Result<()> {
        let initial = claim.checkpoint.is_none()
            && matches!(
                claim.material,
                MailMaterial::Credentials {
                    bootstrap_password: Some(_),
                    ..
                }
            );
        let name = self.name(claim);
        let volume = format!("{name}-data");
        match self.docker.inspect_volume(&volume).await {
            Ok(v) => {
                self.owner(&v.labels, claim)?;
                if v.driver != "local" || !v.options.is_empty() {
                    return Err(ExecutionError::Ownership);
                }
            }
            Err(e) if missing(&e) && initial => {
                authority.guard(claim).await?;
                self.docker
                    .create_volume(VolumeCreateRequest {
                        name: Some(volume.clone()),
                        driver: Some("local".into()),
                        labels: Some(self.labels(claim)?),
                        ..Default::default()
                    })
                    .await
                    .map_err(|_| ExecutionError::Runtime)?;
            }
            Err(_) => return Err(ExecutionError::Ownership),
        }
        if initial {
            self.initialize(claim, authority).await?;
        }
        match self
            .docker
            .inspect_network(&name, None::<InspectNetworkOptions>)
            .await
        {
            Ok(n) => {
                self.owner(n.labels.as_ref().ok_or(ExecutionError::Ownership)?, claim)?;
                if n.internal != Some(true)
                    || n.driver.as_deref() != Some("bridge")
                    || n.containers.as_ref().is_some_and(|peers| {
                        peers
                            .values()
                            .any(|peer| peer.name.as_deref() != Some(&name))
                    })
                    || n.options
                        .as_ref()
                        .is_some_and(|options| !options.is_empty())
                {
                    return Err(ExecutionError::Ownership);
                }
            }
            Err(e) if missing(&e) && initial => {
                authority.guard(claim).await?;
                self.docker
                    .create_network(NetworkCreateRequest {
                        name,
                        internal: Some(true),
                        driver: Some("bridge".into()),
                        labels: Some(self.labels(claim)?),
                        ..Default::default()
                    })
                    .await
                    .map_err(|_| ExecutionError::Runtime)?;
            }
            Err(_) => return Err(ExecutionError::Ownership),
        }
        Ok(())
    }
    async fn create(
        &self,
        claim: &MailClaim,
        authority: &dyn MailAuthority,
        recovery: bool,
    ) -> Result<()> {
        authority.guard(claim).await?;
        self.docker
            .create_container(
                Some(
                    CreateContainerOptionsBuilder::default()
                        .name(&self.name(claim))
                        .build(),
                ),
                self.config(claim, recovery)?,
            )
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        Ok(())
    }
    async fn start(&self, claim: &MailClaim, authority: &dyn MailAuthority) -> Result<()> {
        let container = self
            .container(claim)
            .await?
            .ok_or(ExecutionError::Ownership)?;
        if container.state.as_ref().and_then(|s| s.running) == Some(true) {
            return Ok(());
        }
        let id = container.id.ok_or(ExecutionError::Ownership)?;
        authority.guard(claim).await?;
        self.docker
            .start_container(&id, None::<StartContainerOptions>)
            .await
            .map_err(|_| ExecutionError::Runtime)
    }
    async fn stop(&self, claim: &MailClaim, authority: &dyn MailAuthority) -> Result<()> {
        let container = self
            .container(claim)
            .await?
            .ok_or(ExecutionError::Ownership)?;
        if container.state.as_ref().and_then(|s| s.running) == Some(false) {
            return Ok(());
        }
        let id = container.id.ok_or(ExecutionError::Ownership)?;
        authority.guard(claim).await?;
        self.docker
            .stop_container(
                &id,
                Some(StopContainerOptionsBuilder::default().t(3).build()),
            )
            .await
            .map_err(|_| ExecutionError::Runtime)
    }
    async fn address(&self, claim: &MailClaim) -> Result<(String, Ipv4Addr)> {
        let c = self
            .container(claim)
            .await?
            .ok_or(ExecutionError::Runtime)?;
        let ip = c
            .network_settings
            .and_then(|n| n.networks)
            .and_then(|mut n| n.remove(&self.name(claim)))
            .and_then(|n| n.ip_address)
            .and_then(|ip| ip.parse::<Ipv4Addr>().ok())
            .filter(|ip| ip.is_private())
            .ok_or(ExecutionError::Ownership)?;
        Ok((c.id.ok_or(ExecutionError::Runtime)?, ip))
    }
    fn proof(
        &self,
        claim: &MailClaim,
        id: String,
        running: bool,
        pin: Option<String>,
        ip: Option<Ipv4Addr>,
    ) -> MailProof {
        MailProof {
            runtime_id: id,
            namespace: format!("wd-mail-{}", claim.spec.customer_id),
            revision: claim.spec.revision,
            running,
            edition: MailEdition::Community,
            recovery_disabled: true,
            submission_certificate_sha256: pin,
            private_ipv4: ip,
        }
    }
}
#[async_trait::async_trait]
impl MailRuntime for DockerMailRuntime {
    async fn capabilities(&self) -> Result<Option<MailCapabilities>> {
        Ok(Some(MailCapabilities {
            version: 1,
            image: self.image.clone(),
        }))
    }
    async fn execute(&self, claim: &MailClaim, authority: &dyn MailAuthority) -> Result<MailProof> {
        claim
            .validate(chrono::Utc::now())
            .map_err(|_| ExecutionError::Denied)?;
        if claim.spec.image != self.image {
            return Err(ExecutionError::Ownership);
        }
        authority.guard(claim).await?;
        if claim.spec.action == MailAction::Suspend {
            let Some(c) = self.container(claim).await? else {
                return Ok(self.proof(claim, "unallocated".into(), false, None, None));
            };
            if self.recovery(&c) {
                return Err(ExecutionError::Ownership);
            }
            if c.state.as_ref().and_then(|s| s.running) == Some(true) {
                self.stop(claim, authority).await?;
            }
            return Ok(self.proof(
                claim,
                c.id.ok_or(ExecutionError::Runtime)?,
                false,
                None,
                None,
            ));
        }
        self.infrastructure(claim, authority).await?;
        if self.container(claim).await?.is_none() {
            self.create(
                claim,
                authority,
                matches!(
                    claim.material,
                    MailMaterial::Credentials {
                        bootstrap_password: Some(_),
                        ..
                    }
                ),
            )
            .await?;
        }
        self.start(claim, authority).await?;
        self.ensure_ready(claim, authority).await
    }
}
