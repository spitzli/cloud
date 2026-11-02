//! Isolated concrete workload recovery over the explicitly configured owned Engine.
use super::config::{RecoveryTarget, TargetConfiguration};
use crate::storage::{StorageClient, StorageConfig, StoragePacket};
use bollard::{Docker, models::*, query_parameters::*};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::{
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};
use webdock_supervisor::{ExecutionError, Result, RuntimeSpec};
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoppedApplication {
    pub runtime_id: String,
    pub wanted_running: bool,
    pub environment: BTreeMap<String, String>,
}
pub async fn connect(socket: &std::path::Path) -> Result<Docker> {
    let docker = Docker::connect_with_unix(
        socket.to_str().ok_or(ExecutionError::Ownership)?,
        10,
        bollard::API_DEFAULT_VERSION,
    )
    .map_err(|_| ExecutionError::Runtime)?
    .negotiate_version()
    .await
    .map_err(|_| ExecutionError::Runtime)?;
    Ok(docker)
}
fn environment(values: &[String]) -> Result<BTreeMap<String, String>> {
    let allowed = [
        "NTFY_BASE_URL",
        "NTFY_LISTEN_HTTP",
        "NTFY_LISTEN_HTTPS",
        "NTFY_CACHE_FILE",
        "NTFY_CACHE_DURATION",
        "NTFY_AUTH_FILE",
        "NTFY_AUTH_DEFAULT_ACCESS",
        "NTFY_ATTACHMENT_CACHE_DIR",
        "NTFY_ATTACHMENT_EXPIRY_DURATION",
        "NTFY_CERT_FILE",
        "NTFY_KEY_FILE",
        "NTFY_LOG_LEVEL",
        "NTFY_BEHIND_PROXY",
    ];
    let mut output = BTreeMap::new();
    for value in values {
        let (name, value) = value
            .split_once('=')
            .ok_or(ExecutionError::Invalid("ntfy environment"))?;
        if [
            "PATH",
            "HOME",
            "HOSTNAME",
            "TZ",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
        ]
        .contains(&name)
        {
            continue;
        }
        if !allowed.contains(&name) || value.len() > 4096 || value.contains('\0') {
            return Err(ExecutionError::Unsupported(
                "ntfy external/mutable configuration",
            ));
        }
        output.insert(name.into(), value.into());
    }
    for (name, want) in [
        ("NTFY_CACHE_FILE", "/data/cache.db"),
        ("NTFY_AUTH_FILE", "/data/auth.db"),
        ("NTFY_AUTH_DEFAULT_ACCESS", "deny-all"),
        ("NTFY_ATTACHMENT_CACHE_DIR", "/data/attachments"),
    ] {
        if output.get(name).map(String::as_str) != Some(want) {
            return Err(ExecutionError::Invalid("ntfy consistency profile"));
        }
    }
    if output
        .get("NTFY_BASE_URL")
        .is_none_or(|u| !u.starts_with("https://"))
    {
        return Err(ExecutionError::Invalid("ntfy preserved logical TLS origin"));
    }
    Ok(output)
}
pub async fn stop_application(
    docker: &Docker,
    spec: &RuntimeSpec,
    uid: &str,
    pool: &StorageConfig,
    fence: &super::effect_fence::SameFencePermit,
) -> Result<StoppedApplication> {
    fence.source(spec.resource_id.as_str(), spec.revision, uid)?;
    spec.validate()?;
    if spec.arguments != ["serve"]
        || !spec.mounts.is_empty()
        || spec.plugin.is_some()
        || ![
            "binwiederhier/ntfy@sha256:",
            "docker.io/binwiederhier/ntfy@sha256:",
        ]
        .iter()
        .any(|prefix| spec.image.starts_with(prefix))
    {
        return Err(ExecutionError::Unsupported(
            "only the ntfy local SQLite profile is qualified",
        ));
    }
    let inspected = docker
        .inspect_container(&spec.name(), None::<InspectContainerOptions>)
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    let cfg = inspected.config.as_ref().ok_or(ExecutionError::Ownership)?;
    let labels = cfg.labels.clone().unwrap_or_default().into_iter().collect();
    spec.check_owner(&labels)?;
    if inspected.id.as_deref() != Some(uid)
        || cfg.image.as_deref() != Some(&spec.image)
        || cfg.user.as_deref() != Some("65532:65532")
        || cfg.cmd.as_ref() != Some(&spec.arguments)
    {
        return Err(ExecutionError::Ownership);
    }
    let volume = docker
        .inspect_volume(&format!("{}-data", spec.name()))
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    crate::docker::verify_quota_volume(&volume, spec, pool)?;
    let captured = environment(cfg.env.as_deref().unwrap_or_default())?;
    let wanted = inspected.state.as_ref().and_then(|s| s.running) == Some(true);
    if wanted {
        fence.source(spec.resource_id.as_str(), spec.revision, uid)?;
        docker
            .stop_container(
                uid,
                Some(StopContainerOptions {
                    t: Some(10),
                    ..Default::default()
                }),
            )
            .await
            .map_err(|_| ExecutionError::Runtime)?;
    }
    let stopped = docker
        .inspect_container(uid, None::<InspectContainerOptions>)
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    if stopped.state.as_ref().and_then(|s| s.running) != Some(false) {
        return Err(ExecutionError::Ownership);
    }
    // A volume alias/second writer must not survive the source stop.
    let all = docker
        .list_containers(Some(ListContainersOptions {
            all: true,
            ..Default::default()
        }))
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    for candidate in all {
        let Some(id) = candidate.id else { continue };
        let container = docker
            .inspect_container(&id, None::<InspectContainerOptions>)
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        if (id != uid || container.state.as_ref().and_then(|s| s.running) == Some(true))
            && container.mounts.as_ref().is_some_and(|mounts| {
                mounts.iter().any(|m| {
                    m.name.as_deref() == Some(&volume.name)
                        || m.source.as_deref() == volume.options.get("device").map(String::as_str)
                })
            })
        {
            return Err(ExecutionError::Ownership);
        }
    }
    Ok(StoppedApplication {
        runtime_id: uid.into(),
        wanted_running: wanted,
        environment: captured,
    })
}
pub async fn resume_application(
    docker: &Docker,
    spec: &RuntimeSpec,
    stopped: &StoppedApplication,
    fence: &super::effect_fence::SameFencePermit,
) -> Result<()> {
    fence.source(
        spec.resource_id.as_str(),
        spec.revision,
        &stopped.runtime_id,
    )?;
    let current = docker
        .inspect_container(&stopped.runtime_id, None::<InspectContainerOptions>)
        .await
        .map_err(|_| ExecutionError::Ownership)?;
    let cfg = current.config.as_ref().ok_or(ExecutionError::Ownership)?;
    spec.check_owner(&cfg.labels.clone().unwrap_or_default().into_iter().collect())?;
    if current.id.as_deref() != Some(&stopped.runtime_id)
        || cfg.image.as_deref() != Some(&spec.image)
        || cfg.cmd.as_ref() != Some(&spec.arguments)
        || current.state.as_ref().and_then(|s| s.running) != Some(false)
    {
        return Err(ExecutionError::Ownership);
    }
    if stopped.wanted_running {
        fence.source(
            spec.resource_id.as_str(),
            spec.revision,
            &stopped.runtime_id,
        )?;
        docker
            .start_container(&stopped.runtime_id, None::<StartContainerOptions>)
            .await
            .map_err(|_| ExecutionError::Runtime)?;
    }
    let observed = docker
        .inspect_container(&stopped.runtime_id, None::<InspectContainerOptions>)
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    if observed.state.as_ref().and_then(|s| s.running) != Some(stopped.wanted_running) {
        return Err(ExecutionError::Runtime);
    }
    Ok(())
}
pub async fn start_restored_application(
    docker: &Docker,
    target: &RecoveryTarget,
    pool: &StorageConfig,
    storage: &StorageClient,
    packet: &StoragePacket,
    environment: &BTreeMap<String, String>,
    generation: u64,
) -> Result<String> {
    let TargetConfiguration::Ntfy { runtime } = &target.configuration else {
        return Err(ExecutionError::Invalid("ntfy target"));
    };
    runtime.validate()?;
    if runtime.resource_id != packet.app_id
        || runtime.project_id != packet.project_id
        || runtime.arguments != ["serve"]
        || runtime.volume.as_ref().and_then(|v| v.size_bytes) != Some(packet.spec.volume_bytes)
    {
        return Err(ExecutionError::Ownership);
    }
    // No preexisting container or Engine volume is overwritten by a restore.
    match docker
        .inspect_container(&runtime.name(), None::<InspectContainerOptions>)
        .await
    {
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => {}
        _ => return Err(ExecutionError::Ownership),
    }
    let (actual, path) = storage.ensure(packet).await?;
    if actual != *pool {
        return Err(ExecutionError::Ownership);
    }
    let name = format!("{}-data", runtime.name());
    let mut labels: HashMap<_, _> = runtime.labels().into_iter().collect();
    labels.insert("webdock.dev/storage-pool".into(), pool.pool_id.clone());
    labels.insert(
        "webdock.dev/storage-bytes".into(),
        packet.spec.volume_bytes.to_string(),
    );
    match docker.inspect_volume(&name).await {
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => {
            docker
                .create_volume(VolumeCreateRequest {
                    name: Some(name.clone()),
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
        Ok(volume) => crate::docker::verify_quota_volume(&volume, runtime, pool)?,
        Err(_) => return Err(ExecutionError::Runtime),
    }
    let mut config = crate::docker::container_config(runtime, generation)?;
    let mut environment = self::environment(
        &environment
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>(),
    )?;
    environment.insert("NTFY_LISTEN_HTTP".into(), ":8080".into());
    environment.remove("NTFY_LISTEN_HTTPS");
    environment.remove("NTFY_CERT_FILE");
    environment.remove("NTFY_KEY_FILE");
    config.env = Some(
        environment
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect(),
    );
    config
        .labels
        .as_mut()
        .ok_or(ExecutionError::Ownership)?
        .insert(
            "webdock.dev/recovery-quarantine".into(),
            target.nonce.clone(),
        );
    let host = config
        .host_config
        .as_mut()
        .ok_or(ExecutionError::Ownership)?;
    host.network_mode = Some("none".into());
    host.restart_policy = Some(RestartPolicy {
        name: Some(RestartPolicyNameEnum::NO),
        ..Default::default()
    });
    let result = docker
        .create_container(
            Some(CreateContainerOptions {
                name: Some(runtime.name()),
                ..Default::default()
            }),
            config,
        )
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    docker
        .start_container(&result.id, None::<StartContainerOptions>)
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    let inspected = docker
        .inspect_container(&result.id, None::<InspectContainerOptions>)
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    if inspected
        .host_config
        .as_ref()
        .and_then(|h| h.network_mode.as_deref())
        != Some("none")
        || inspected.config.as_ref().and_then(|c| c.user.as_deref()) != Some("65532:65532")
    {
        return Err(ExecutionError::Ownership);
    }
    Ok(result.id)
}

pub async fn start_restored_ce(
    docker: &Docker,
    target: &RecoveryTarget,
    pool: &StorageConfig,
    data_mount: &str,
    record: &crate::storage::recovery_pool::RecoveryVolumeRecord,
    approved: &crate::storage::recovery_pool::ApprovedRecoveryVolume,
) -> Result<String> {
    let customer = match &record.role {
        crate::storage::recovery_pool::RecoveryVolumeRole::IncomingCe {
            original_customer_id,
        } => original_customer_id.as_str(),
        _ => return Err(ExecutionError::Ownership),
    };
    let path = record.data_directory.as_path();
    let image = approved.image.as_str();
    if !["/data", "/var/lib/stalwart"].contains(&data_mount)
        || customer.is_empty()
        || !customer.bytes().all(|c| c.is_ascii_digit())
        || !webdock_supervisor::image_digest(image)
        || path
            != pool
                .root
                .join("_mail-recovery")
                .join(&record.resource_id)
                .join("mount/data")
        || record.uid != 65532
        || record.gid != 65532
        || approved.uid != 65532
        || approved.gid != 65532
        || approved.image != image
    {
        return Err(ExecutionError::Ownership);
    }
    let name = format!("wd-recovery-ce-{}-{}", target.nonce, record.resource_id);
    let volume_name = format!("{name}-data");
    match docker
        .inspect_container(&name, None::<InspectContainerOptions>)
        .await
    {
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => {}
        _ => return Err(ExecutionError::Ownership),
    }
    match docker.inspect_volume(&volume_name).await {
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => {}
        _ => return Err(ExecutionError::Ownership),
    }
    let labels = HashMap::from([
        (
            "webdock.dev/recovery-quarantine".into(),
            target.nonce.clone(),
        ),
        ("webdock.mail.customer".into(), customer.into()),
        ("webdock.dev/storage-pool".into(), pool.pool_id.clone()),
    ]);
    docker
        .create_volume(VolumeCreateRequest {
            name: Some(volume_name.clone()),
            driver: Some("local".into()),
            driver_opts: Some(HashMap::from([
                ("type".into(), "none".into()),
                ("o".into(), "bind".into()),
                (
                    "device".into(),
                    path.to_str().ok_or(ExecutionError::Ownership)?.into(),
                ),
            ])),
            labels: Some(labels.clone()),
            ..Default::default()
        })
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    let config = ContainerCreateBody {
        image: Some(image.into()),
        user: Some("65532:65532".into()),
        cmd: Some(vec!["--config".into(), format!("{data_mount}/config.json")]),
        env: Some(vec![
            "STALWART_RECOVERY_MODE=1".into(),
            "STALWART_RECOVERY_MODE_PORT=8080".into(),
            "STALWART_RECOVERY_MODE_LOG_LEVEL=error".into(),
        ]),
        labels: Some(labels),
        host_config: Some(HostConfig {
            network_mode: Some("none".into()),
            readonly_rootfs: Some(true),
            privileged: Some(false),
            cap_drop: Some(vec!["ALL".into()]),
            security_opt: Some(vec!["no-new-privileges:true".into()]),
            memory: Some(approved.memory_bytes as i64),
            memory_swap: Some(approved.memory_bytes as i64),
            nano_cpus: Some(i64::from(approved.cpu_millicores) * 1000000),
            pids_limit: Some(128),
            restart_policy: Some(RestartPolicy {
                name: Some(RestartPolicyNameEnum::NO),
                ..Default::default()
            }),
            tmpfs: Some(HashMap::from([
                (
                    "/tmp".into(),
                    format!(
                        "rw,nodev,nosuid,noexec,size={},mode=1777",
                        approved.tmp_bytes - 1048576
                    ),
                ),
                (
                    "/etc/stalwart".into(),
                    "rw,nodev,nosuid,noexec,size=1048576".into(),
                ),
            ])),
            mounts: Some(vec![Mount {
                target: Some(data_mount.into()),
                source: Some(volume_name),
                typ: Some(MountType::VOLUME),
                read_only: Some(false),
                ..Default::default()
            }]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let created = docker
        .create_container(
            Some(CreateContainerOptions {
                name: Some(name),
                ..Default::default()
            }),
            config,
        )
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    docker
        .start_container(&created.id, None::<StartContainerOptions>)
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    let actual = docker
        .inspect_container(&created.id, None::<InspectContainerOptions>)
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    if actual
        .host_config
        .as_ref()
        .and_then(|h| h.network_mode.as_deref())
        != Some("none")
        || actual
            .config
            .as_ref()
            .and_then(|c| c.env.as_ref())
            .is_none_or(|v| {
                !v.contains(&"STALWART_RECOVERY_MODE=1".into())
                    || v.iter().any(|e| e.starts_with("STALWART_RECOVERY_ADMIN="))
            })
    {
        return Err(ExecutionError::Ownership);
    }
    Ok(created.id)
}

/// A private inspection sidecar shares only the restored network-none namespace.
/// The readonly gateway's TLS key/config directory contains no source master keys.
pub async fn start_gateway(
    docker: &Docker,
    target: &RecoveryTarget,
    application_id: &str,
    folder: &Path,
    mail: bool,
    remaining_seconds: u64,
) -> Result<String> {
    let directory = folder.join(format!(
        "gateway-{}",
        &application_id[..16.min(application_id.len())]
    ));
    std::fs::create_dir(&directory).map_err(|_| ExecutionError::Conflict)?;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
        .map_err(|_| ExecutionError::Runtime)?;
    for (name, path) in [
        ("server.crt", &target.tls_certificate),
        ("server.key", &target.tls_key),
    ] {
        let bytes = super::config::private(path, 65536)?;
        let output = directory.join(name);
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&output)
            .map_err(|_| ExecutionError::Runtime)?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| ExecutionError::Runtime)?;
        std::os::unix::fs::chown(output, Some(65532), Some(65532))
            .map_err(|_| ExecutionError::Runtime)?;
    }
    let config = serde_json::json!({"profile":if mail{"mail"}else{"ntfy"},"certificate":"/recovery/server.crt","privateKey":"/recovery/server.key","port":8443,"upstreamPort":8080,"deadlineSeconds":remaining_seconds.clamp(1,900)});
    let path = directory.join("gateway.json");
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .map_err(|_| ExecutionError::Runtime)?;
    file.write_all(&serde_json::to_vec(&config).map_err(|_| ExecutionError::Runtime)?)
        .and_then(|()| file.sync_all())
        .map_err(|_| ExecutionError::Runtime)?;
    std::os::unix::fs::chown(&path, Some(65532), Some(65532))
        .map_err(|_| ExecutionError::Runtime)?;
    std::os::unix::fs::chown(&directory, Some(65532), Some(65532))
        .map_err(|_| ExecutionError::Runtime)?;
    let name = format!(
        "wd-recovery-inspect-{}-{}",
        target.nonce,
        &application_id[..12.min(application_id.len())]
    );
    let config = ContainerCreateBody {
        image: Some(target.gateway_image.clone()),
        entrypoint: Some(vec!["/usr/local/bin/webdock-recovery-gateway".into()]),
        cmd: Some(vec!["/recovery/gateway.json".into()]),
        user: Some("65532:65532".into()),
        labels: Some(HashMap::from([(
            "webdock.dev/recovery-quarantine".into(),
            target.nonce.clone(),
        )])),
        host_config: Some(HostConfig {
            network_mode: Some(format!("container:{application_id}")),
            readonly_rootfs: Some(true),
            cap_drop: Some(vec!["ALL".into()]),
            security_opt: Some(vec!["no-new-privileges:true".into()]),
            memory: Some(134217728),
            memory_swap: Some(134217728),
            nano_cpus: Some(100000000),
            pids_limit: Some(32),
            mounts: Some(vec![Mount {
                target: Some("/recovery".into()),
                source: Some(directory.to_str().ok_or(ExecutionError::Ownership)?.into()),
                typ: Some(MountType::BIND),
                read_only: Some(true),
                ..Default::default()
            }]),
            restart_policy: Some(RestartPolicy {
                name: Some(RestartPolicyNameEnum::NO),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    let created = docker
        .create_container(
            Some(CreateContainerOptions {
                name: Some(name),
                ..Default::default()
            }),
            config,
        )
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    docker
        .start_container(&created.id, None::<StartContainerOptions>)
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    Ok(created.id)
}

async fn identity_exec(docker: &Docker, container: &str, command: Vec<String>) -> Result<String> {
    use futures_util::StreamExt;
    let exec = docker
        .create_exec(
            container,
            bollard::exec::CreateExecOptions {
                attach_stdout: Some(true),
                attach_stderr: Some(false),
                cmd: Some(command),
                ..Default::default()
            },
        )
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    let bollard::exec::StartExecResults::Attached { mut output, .. } = docker
        .start_exec(&exec.id, None)
        .await
        .map_err(|_| ExecutionError::Runtime)?
    else {
        return Err(ExecutionError::Runtime);
    };
    let mut bytes = Vec::new();
    while let Some(chunk) = output.next().await {
        let chunk = chunk.map_err(|_| ExecutionError::Runtime)?;
        bytes.extend_from_slice(&chunk.into_bytes());
        if bytes.len() > 512 {
            return Err(ExecutionError::Runtime);
        }
    }
    if docker
        .inspect_exec(&exec.id)
        .await
        .map_err(|_| ExecutionError::Runtime)?
        .exit_code
        != Some(0)
    {
        return Err(ExecutionError::Runtime);
    }
    String::from_utf8(bytes)
        .map(|v| v.trim().to_owned())
        .map_err(|_| ExecutionError::Runtime)
}
pub async fn start_restored_postgres(
    docker: &Docker,
    target: &RecoveryTarget,
    endpoint: &super::config::PostgresEndpoint,
    record: &crate::storage::recovery_pool::RecoveryVolumeRecord,
    approved: &crate::storage::recovery_pool::ApprovedRecoveryVolume,
) -> Result<String> {
    if !endpoint
        .host_address
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
        || endpoint.server_name != "localhost"
        || endpoint.username != "postgres"
        || approved.uid != record.uid
        || approved.gid != record.gid
        || approved.bytes != record.bytes
        || approved.role != record.role
        || !matches!(
            record.role,
            crate::storage::recovery_pool::RecoveryVolumeRole::CoreLedgerPostgres {}
                | crate::storage::recovery_pool::RecoveryVolumeRole::PrivateMailPostgres {}
        )
    {
        return Err(ExecutionError::Ownership);
    }
    let image = docker
        .inspect_image(&approved.image)
        .await
        .map_err(|_| ExecutionError::Ownership)?;
    let configuration = image.config.ok_or(ExecutionError::Ownership)?;
    let variables = configuration
        .env
        .unwrap_or_default()
        .into_iter()
        .filter_map(|line| {
            line.split_once('=')
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
        })
        .collect::<BTreeMap<_, _>>();
    let pgdata = variables.get("PGDATA").ok_or(ExecutionError::Ownership)?;
    let volumes = configuration.volumes.ok_or(ExecutionError::Ownership)?;
    if volumes.len() != 1 {
        return Err(ExecutionError::Ownership);
    }
    let data_mount = volumes.first().ok_or(ExecutionError::Ownership)?;
    if !std::path::Path::new(pgdata).starts_with(data_mount) || !data_mount.starts_with('/') {
        return Err(ExecutionError::Ownership);
    }
    let name = format!("wd-recovery-pg-{}-{}", target.nonce, record.resource_id);
    match docker
        .inspect_container(&name, None::<InspectContainerOptions>)
        .await
    {
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => {}
        _ => return Err(ExecutionError::Ownership),
    }
    let probe_name = format!("{name}-identity");
    let probe = docker
        .create_container(
            Some(CreateContainerOptions {
                name: Some(probe_name),
                ..Default::default()
            }),
            ContainerCreateBody {
                image: Some(approved.image.clone()),
                user: Some(format!("{}:{}", approved.uid, approved.gid)),
                entrypoint: Some(vec!["/bin/sleep".into()]),
                cmd: Some(vec!["15".into()]),
                labels: Some(HashMap::from([(
                    "webdock.dev/recovery-quarantine".into(),
                    target.nonce.clone(),
                )])),
                host_config: Some(HostConfig {
                    network_mode: Some("none".into()),
                    readonly_rootfs: Some(true),
                    cap_drop: Some(vec!["ALL".into()]),
                    security_opt: Some(vec!["no-new-privileges:true".into()]),
                    memory: Some(134217728),
                    memory_swap: Some(134217728),
                    nano_cpus: Some(100000000),
                    pids_limit: Some(16),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    let identity = async {
        docker
            .start_container(&probe.id, None::<StartContainerOptions>)
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        let uid = identity_exec(
            docker,
            &probe.id,
            vec!["id".into(), "-u".into(), "postgres".into()],
        )
        .await?;
        let gid = identity_exec(
            docker,
            &probe.id,
            vec!["id".into(), "-g".into(), "postgres".into()],
        )
        .await?;
        let version = identity_exec(
            docker,
            &probe.id,
            vec!["postgres".into(), "--version".into()],
        )
        .await?;
        if uid != approved.uid.to_string()
            || gid != approved.gid.to_string()
            || !version.contains("PostgreSQL) 18.")
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(())
    }
    .await;
    docker
        .remove_container(
            &probe.id,
            Some(RemoveContainerOptions {
                force: true,
                v: true,
                ..Default::default()
            }),
        )
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    identity?;
    let password = String::from_utf8(super::config::private(&endpoint.password_file, 4096)?)
        .map_err(|_| ExecutionError::Ownership)?;
    let part = approved.tmp_bytes / 4;
    let temporary = format!(
        "rw,nodev,nosuid,noexec,size={part},mode=0700,uid={},gid={}",
        approved.uid, approved.gid
    );
    let created = docker
        .create_container(
            Some(CreateContainerOptions {
                name: Some(name),
                ..Default::default()
            }),
            ContainerCreateBody {
                image: Some(approved.image.clone()),
                user: Some(format!("{}:{}", approved.uid, approved.gid)),
                env: Some(vec![
                    format!("POSTGRES_PASSWORD={}", password.trim_end()),
                    "POSTGRES_USER=postgres".into(),
                    format!("PGDATA={pgdata}"),
                ]),
                cmd: Some(vec![
                    "postgres".into(),
                    "-p".into(),
                    endpoint.port.to_string(),
                    "-h".into(),
                    "127.0.0.1".into(),
                    "-c".into(),
                    "ssl=on".into(),
                    "-c".into(),
                    format!("ssl_cert_file={data_mount}/.operator-tls/server.crt"),
                    "-c".into(),
                    format!("ssl_key_file={data_mount}/.operator-tls/server.key"),
                    "-c".into(),
                    "max_connections=20".into(),
                    "-c".into(),
                    "shared_buffers=32MB".into(),
                    "-c".into(),
                    "max_wal_size=64MB".into(),
                    "-c".into(),
                    "min_wal_size=32MB".into(),
                ]),
                labels: Some(HashMap::from([
                    (
                        "webdock.dev/recovery-quarantine".into(),
                        target.nonce.clone(),
                    ),
                    (
                        "webdock.dev/recovery-resource".into(),
                        record.resource_id.clone(),
                    ),
                    (
                        "webdock.dev/recovery-reservation".into(),
                        record.reservation_id.clone(),
                    ),
                ])),
                host_config: Some(HostConfig {
                    network_mode: Some("host".into()),
                    readonly_rootfs: Some(true),
                    cap_drop: Some(vec!["ALL".into()]),
                    security_opt: Some(vec!["no-new-privileges:true".into()]),
                    memory: Some(approved.memory_bytes as i64),
                    memory_swap: Some(approved.memory_bytes as i64),
                    nano_cpus: Some(i64::from(approved.cpu_millicores) * 1000000),
                    pids_limit: Some(128),
                    shm_size: Some((approved.tmp_bytes / 2) as i64),
                    tmpfs: Some(HashMap::from([
                        ("/tmp".into(), temporary.clone()),
                        ("/var/run/postgresql".into(), temporary),
                    ])),
                    mounts: Some(vec![Mount {
                        target: Some(data_mount.clone()),
                        source: Some(
                            record
                                .data_directory
                                .to_str()
                                .ok_or(ExecutionError::Ownership)?
                                .into(),
                        ),
                        typ: Some(MountType::BIND),
                        read_only: Some(false),
                        ..Default::default()
                    }]),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    docker
        .start_container(&created.id, None::<StartContainerOptions>)
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    let actual = docker
        .inspect_container(&created.id, None::<InspectContainerOptions>)
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    if actual.config.as_ref().and_then(|c| c.user.as_deref())
        != Some(format!("{}:{}", approved.uid, approved.gid).as_str())
    {
        return Err(ExecutionError::Ownership);
    }
    Ok(created.id)
}
