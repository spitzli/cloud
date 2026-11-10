//! Matched installation Mail recovery: canonical Core, private PG/payloads and offline CE.
use super::{
    NativeRecoveryExecutor, RecoveryAuthority, archive,
    config::{
        self, PostgresEndpoint, RecoveryProfileConfig, RecoverySource, RecoveryTarget,
        TargetConfiguration,
    },
};
use crate::storage::StorageGuard;
use base64::Engine;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use spitzli_agent_protocol::recovery::RecoveryClaim;
use spitzli_supervisor::{ExecutionError, Result};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

fn name(value: &str) -> Result<&str> {
    if value.is_empty()
        || value.len() > 63
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_')
    {
        return Err(ExecutionError::Ownership);
    }
    Ok(value)
}
fn url(endpoint: &PostgresEndpoint) -> Result<String> {
    let mut url = url::Url::parse("postgresql://localhost").map_err(|_| ExecutionError::Runtime)?;
    url.set_username(&endpoint.username)
        .map_err(|_| ExecutionError::Ownership)?;
    let password = String::from_utf8(config::private(&endpoint.password_file, 4096)?)
        .map_err(|_| ExecutionError::Ownership)?;
    url.set_password(Some(password.trim_end()))
        .map_err(|_| ExecutionError::Ownership)?;
    url.set_host(Some(&endpoint.server_name))
        .map_err(|_| ExecutionError::Ownership)?;
    url.set_port(Some(endpoint.port))
        .map_err(|_| ExecutionError::Ownership)?;
    url.set_path(&endpoint.database);
    url.query_pairs_mut()
        .append_pair("sslmode", "verify-full")
        .append_pair(
            "sslrootcert",
            endpoint
                .authority
                .to_str()
                .ok_or(ExecutionError::Ownership)?,
        );
    Ok(url.to_string())
}
async fn command(
    endpoint: &PostgresEndpoint,
    program: &str,
    args: &[&str],
    input: Option<&[u8]>,
) -> Result<Vec<u8>> {
    use tokio::io::AsyncWriteExt;
    endpoint.validate()?;
    let password = String::from_utf8(config::private(&endpoint.password_file, 4096)?)
        .map_err(|_| ExecutionError::Ownership)?;
    config::private(&endpoint.authority, 65536)?;
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .env("PGHOST", &endpoint.server_name)
        .env("PGHOSTADDR", &endpoint.host_address)
        .env("PGPORT", endpoint.port.to_string())
        .env("PGDATABASE", &endpoint.database)
        .env("PGUSER", &endpoint.username)
        .env("PGPASSWORD", password.trim_end())
        .env("PGSSLMODE", "verify-full")
        .env("PGSSLROOTCERT", &endpoint.authority)
        .env("PGAPPNAME", "webdock-native-recovery")
        .env("PGCONNECT_TIMEOUT", "5")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|_| ExecutionError::Runtime)?;
    if let Some(input) = input {
        child
            .stdin
            .take()
            .ok_or(ExecutionError::Runtime)?
            .write_all(input)
            .await
            .map_err(|_| ExecutionError::Runtime)?;
    }
    let output = tokio::time::timeout(Duration::from_secs(120), child.wait_with_output())
        .await
        .map_err(|_| ExecutionError::Expired)?
        .map_err(|_| ExecutionError::Runtime)?;
    if !output.status.success() {
        return Err(ExecutionError::Runtime);
    }
    Ok(output.stdout)
}
async fn sql(endpoint: &PostgresEndpoint, statement: &str) -> Result<String> {
    let bytes = command(
        endpoint,
        "/usr/bin/psql",
        &[
            "-X",
            "-q",
            "-A",
            "-t",
            "--no-password",
            "-v",
            "ON_ERROR_STOP=1",
        ],
        Some(statement.as_bytes()),
    )
    .await?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err(ExecutionError::Runtime);
    }
    String::from_utf8(bytes)
        .map(|s| s.trim().to_owned())
        .map_err(|_| ExecutionError::Runtime)
}
async fn dump(endpoint: &PostgresEndpoint, output: &Path) -> Result<()> {
    command(
        endpoint,
        "/usr/bin/pg_dump",
        &[
            "--format=custom",
            "--no-password",
            "--no-publications",
            "--no-subscriptions",
            "--file",
            output.to_str().ok_or(ExecutionError::Ownership)?,
        ],
        None,
    )
    .await?;
    std::fs::set_permissions(output, std::fs::Permissions::from_mode(0o600))
        .map_err(|_| ExecutionError::Runtime)?;
    std::fs::File::open(output)
        .and_then(|f| f.sync_all())
        .map_err(|_| ExecutionError::Runtime)
}
fn directory(path: &Path) -> Result<()> {
    std::fs::create_dir(path).map_err(|_| ExecutionError::Conflict)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|_| ExecutionError::Runtime)
}
fn copy_tree(input: &Path, output: &Path, guard: &StorageGuard, max: u64, uid: u32) -> Result<u64> {
    directory(output)?;
    let mut pending = vec![(input.to_owned(), output.to_owned())];
    let mut total = 0u64;
    let mut count = 0;
    while let Some((source, target)) = pending.pop() {
        for entry in std::fs::read_dir(&source).map_err(|_| ExecutionError::Runtime)? {
            guard.check()?;
            let entry = entry.map_err(|_| ExecutionError::Runtime)?;
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path).map_err(|_| ExecutionError::Runtime)?;
            let destination = target.join(entry.file_name());
            if metadata.file_type().is_symlink() {
                return Err(ExecutionError::Ownership);
            }
            if metadata.is_dir() {
                directory(&destination)?;
                std::os::unix::fs::chown(&destination, Some(uid), Some(uid))
                    .map_err(|_| ExecutionError::Runtime)?;
                pending.push((path, destination));
                continue;
            }
            if !metadata.is_file() || metadata.nlink() != 1 {
                return Err(ExecutionError::Ownership);
            }
            total = total
                .checked_add(metadata.len())
                .ok_or(ExecutionError::Runtime)?;
            count += 1;
            if total > max || count > 100000 {
                return Err(ExecutionError::Invalid("offline CE snapshot capacity"));
            }
            let mut input = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
                .open(path)
                .map_err(|_| ExecutionError::Runtime)?;
            let mut output = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&destination)
                .map_err(|_| ExecutionError::Runtime)?;
            let mut buffer = [0; 65536];
            loop {
                guard.check()?;
                let n = input
                    .read(&mut buffer)
                    .map_err(|_| ExecutionError::Runtime)?;
                if n == 0 {
                    break;
                }
                output
                    .write_all(&buffer[..n])
                    .map_err(|_| ExecutionError::Runtime)?;
            }
            output.sync_all().map_err(|_| ExecutionError::Runtime)?;
            std::os::unix::fs::chown(&destination, Some(uid), Some(uid))
                .map_err(|_| ExecutionError::Runtime)?;
        }
    }
    Ok(total)
}
async fn selected(
    docker: &bollard::Docker,
    labels: &BTreeMap<String, String>,
) -> Result<Vec<bollard::models::ContainerInspectResponse>> {
    use bollard::query_parameters::*;
    let filters = std::collections::HashMap::from([(
        "label".into(),
        labels
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>(),
    )]);
    let values = docker
        .list_containers(Some(ListContainersOptions {
            all: true,
            filters: Some(filters),
            ..Default::default()
        }))
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    if values.len() > 200 {
        return Err(ExecutionError::Invalid("Mail recovery participant limit"));
    }
    let mut found = Vec::new();
    for value in values {
        let id = value.id.ok_or(ExecutionError::Ownership)?;
        let inspected = docker
            .inspect_container(&id, None::<InspectContainerOptions>)
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        if !labels.iter().all(|(k, v)| {
            inspected
                .config
                .as_ref()
                .and_then(|c| c.labels.as_ref())
                .and_then(|l| l.get(k))
                == Some(v)
        }) {
            return Err(ExecutionError::Ownership);
        }
        found.push(inspected);
    }
    Ok(found)
}
async fn stop_selected(
    docker: &bollard::Docker,
    labels: &BTreeMap<String, String>,
) -> Result<Vec<Value>> {
    use bollard::query_parameters::*;
    let mut stopped = Vec::new();
    for participant in selected(docker, labels).await? {
        let id = participant.id.ok_or(ExecutionError::Ownership)?;
        let cfg = participant.config.ok_or(ExecutionError::Ownership)?;
        let wanted = participant.state.as_ref().and_then(|s| s.running) == Some(true);
        if wanted {
            docker
                .stop_container(
                    &id,
                    Some(StopContainerOptions {
                        t: Some(60),
                        ..Default::default()
                    }),
                )
                .await
                .map_err(|_| ExecutionError::Runtime)?;
        }
        let actual = docker
            .inspect_container(&id, None::<InspectContainerOptions>)
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        if actual.state.as_ref().and_then(|s| s.running) != Some(false) {
            return Err(ExecutionError::Ownership);
        }
        stopped.push(json!({"id":id,"image":cfg.image,"wantedRunning":wanted,"labels":cfg.labels}));
    }
    Ok(stopped)
}
pub async fn backup(
    executor: &NativeRecoveryExecutor,
    c: &RecoveryClaim,
    authority: &dyn RecoveryAuthority,
    profile: &RecoveryProfileConfig,
    _plan: &Value,
    reservation: &super::RecoveryReservationReceipt,
) -> Result<Value> {
    let RecoverySource::Mail {
        core,
        metadata,
        runtime_labels,
        ce_labels,
    } = &profile.source
    else {
        return Err(ExecutionError::Ownership);
    };
    let cancelled = Arc::new(AtomicBool::new(false));
    let guard = StorageGuard::new(c.lease.expires_at, cancelled.clone());
    let fencing_plan = authority
        .checkpoint(c, "fencing", json!({"phase":"canonical-fencing"}))
        .await?;
    let effect_drain = executor.mail_effect_fence.close_and_drain().await?;
    let identity = &fencing_plan["sourceFence"]["agentIdentity"];
    let mut participants = vec![
        json!({"kind":"guarded-agent","identity":identity["identity"],"uid":identity["uid"],"ownerSha256":config::fingerprint(identity)?,"agentGeneration":identity["generation"],"effectGuardVersion":3}),
    ];
    if fencing_plan["sourceFence"]["enrolledExecutors"]
        .as_array()
        .is_none_or(|agents| {
            agents
                .iter()
                .any(|agent| agent["identity"] != identity["identity"])
        })
    {
        return Err(ExecutionError::Unsupported(
            "known legacy executors must be explicitly stopped before source capture",
        ));
    }
    #[cfg(feature = "kubernetes")]
    let kube_source = if let Some(source) = &executor.config.kubernetes_source {
        Some((source, super::kubernetes::connect(source).await?))
    } else {
        None
    };
    #[cfg(feature = "kubernetes")]
    let kube_observed = if let Some((source, client)) = &kube_source {
        super::kubernetes::observe_mail(
            client.clone(),
            source,
            &executor.config.installation,
            &fencing_plan,
        )
        .await?
    } else {
        Vec::new()
    };
    #[cfg(feature = "kubernetes")]
    for observed in &kube_observed {
        let runtime = fencing_plan["source"]["runtimes"]
            .as_array()
            .and_then(|runtimes| {
                runtimes
                    .iter()
                    .find(|r| r["namespace"] == observed.namespace)
            });
        participants.push(json!({"kind":if observed.ce{"ce"}else{"runtime"},"identity":format!("deployment:{}/{}",observed.namespace,observed.name),"uid":observed.uid,"resourceVersion":observed.resource_version,"templateSha256":observed.template_sha256,"customerID":observed.customer_id,"serviceRevision":observed.service_revision,"native":observed.service_revision.is_some(),"ownerSha256":config::fingerprint(observed)?,"namespace":observed.namespace,"unit":observed.labels.get("webdock.dev/unit"),"runtimeGeneration":runtime.map(|r|r["generation"].clone()),"image":observed.image}));
    }
    #[cfg(not(feature = "kubernetes"))]
    let _ = &mut participants;
    if participants.len() == 1
        && (fencing_plan["source"]["runtimes"]
            .as_array()
            .is_some_and(|r| !r.is_empty())
            || fencing_plan["source"]["workloads"]
                .as_array()
                .is_some_and(|r| !r.is_empty()))
    {
        return Err(ExecutionError::Unsupported(
            "all canonical Mail participants must be observed before admitting source fence",
        ));
    }
    let admitted = authority
        .checkpoint(c, "source-admit", json!({"participants":participants}))
        .await?;
    let plan_sha = admitted["sourceFence"]["fence"]["plan_sha256"]
        .as_str()
        .ok_or(ExecutionError::Ownership)?
        .to_owned();
    let private = sql(
        metadata,
        "SELECT epoch::text||'|'||revision::text FROM mail_recovery_control WHERE singleton;",
    )
    .await?;
    let (private_epoch, private_revision) =
        private.split_once('|').ok_or(ExecutionError::Ownership)?;
    let private_epoch = uuid::Uuid::parse_str(private_epoch)
        .map_err(|_| ExecutionError::Ownership)?
        .to_string();
    let private_revision = private_revision
        .parse::<i64>()
        .map_err(|_| ExecutionError::Ownership)?;
    let private_generation = c
        .spec
        .operation_id
        .parse::<i64>()
        .map_err(|_| ExecutionError::Ownership)?;
    if sql(metadata,&format!("UPDATE mail_recovery_control SET state='fenced',operation='{}'::uuid,generation={} WHERE singleton AND epoch='{}'::uuid AND state='active' AND revision={} RETURNING epoch::text;",c.spec.epoch,private_generation,private_epoch,private_revision)).await?!=private_epoch{return Err(ExecutionError::Fenced);}
    #[cfg(feature = "kubernetes")]
    let kube_stopped = if let Some((_, client)) = &kube_source {
        executor
            .guarded(
                c,
                authority,
                cancelled.clone(),
                super::kubernetes::stop_observed(client.clone(), &kube_observed, &guard),
            )
            .await?
    } else {
        Vec::new()
    };
    let runtimes = if executor.config.kubernetes_source.is_none() {
        executor
            .guarded(
                c,
                authority,
                cancelled.clone(),
                stop_selected(&executor.docker, runtime_labels),
            )
            .await?
    } else {
        Vec::new()
    };
    let ce = if executor.config.kubernetes_source.is_none() {
        executor
            .guarded(
                c,
                authority,
                cancelled.clone(),
                stop_selected(&executor.docker, ce_labels),
            )
            .await?
    } else {
        Vec::new()
    };
    let mut outcomes = participants.clone();
    for outcome in &mut outcomes {
        outcome["state"] = json!(if outcome["kind"] == "guarded-agent" {
            "guarded-drained"
        } else {
            "stopped"
        });
        outcome["uncertain"] = json!(false);
    }
    authority
        .checkpoint(
            c,
            "source-drained",
            json!({"planSha256":plan_sha,"allPVCWritersAbsent":true,"participants":outcomes}),
        )
        .await?;
    // The DB fence also terminates residual/pre-gate metadata writers; external CE
    // processes were observed stopped first. Retain uncertain effect custody unchanged.
    let metadata_owner = sql(
        metadata,
        "SELECT pg_get_userbyid(datdba) FROM pg_database WHERE datname=current_database();",
    )
    .await?;
    name(&metadata_owner)?;
    let metadata_was_login = match sql(
        metadata,
        &format!(
            "SELECT rolcanlogin FROM pg_roles WHERE rolname='{}';",
            metadata_owner
        ),
    )
    .await?
    .as_str()
    {
        "t" => true,
        "f" => false,
        _ => return Err(ExecutionError::Ownership),
    };
    if metadata_owner != metadata.username && metadata_was_login {
        sql(
            metadata,
            &format!("ALTER ROLE \"{}\" NOLOGIN;", metadata_owner),
        )
        .await?;
    }
    sql(metadata,"SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid();").await?;
    let plan=authority.checkpoint(c,"fenced",json!({"phase":"all-writers-stopped","runtimes":runtimes,"ce":ce,"privateEpoch":private_epoch})).await?;
    let services = plan["source"]["services"]
        .as_array()
        .ok_or(ExecutionError::Ownership)?;
    let mut incoming: std::collections::BTreeSet<_> = services
        .iter()
        .filter_map(|s| s["customerID"].as_str())
        .collect();
    for workload in plan["source"]["workloads"]
        .as_array()
        .ok_or(ExecutionError::Ownership)?
    {
        if let Some(customer) = workload["customerID"].as_str() {
            incoming.insert(customer);
        }
    }
    let folder = reservation.staging_directory.join("mail-backup");
    directory(&folder)?;
    let data = folder.join("data");
    directory(&data)?;
    executor
        .guarded(
            c,
            authority,
            cancelled.clone(),
            dump(core, &data.join("core.dump")),
        )
        .await?;
    executor
        .guarded(
            c,
            authority,
            cancelled.clone(),
            dump(metadata, &data.join("private.dump")),
        )
        .await?;
    let ce_root = data.join("ce");
    directory(&ce_root)?;
    let mut ce_receipts = Vec::new();
    #[cfg(feature = "kubernetes")]
    if let Some((source, client)) = &kube_source {
        for stopped in kube_stopped.iter().filter(|s| s.ce) {
            let customer = stopped
                .customer_id
                .as_deref()
                .ok_or(ExecutionError::Ownership)?;
            name(customer)?;
            let encrypted_source = folder.join(format!("ce-{customer}.tar"));
            executor
                .guarded_cleanup(
                    c,
                    authority,
                    cancelled.clone(),
                    super::kubernetes::snapshot_ce(
                        client.clone(),
                        source,
                        stopped,
                        &encrypted_source,
                        c.spec.capacity_bytes,
                        &guard,
                    ),
                )
                .await?;
            let output = ce_root.join(customer);
            directory(&output)?;
            archive::unpack(&encrypted_source, &output, &guard, c.spec.capacity_bytes)?;
            ce_receipts.push(json!({"customerID":customer,"image":stopped.image,"sourceRuntimeID":stopped.uid,"sourceVolume":stopped.claim,"dataMount":stopped.data_mount,"namespace":stopped.namespace}));
        }
    }
    for resource in if executor.config.kubernetes_source.is_none() {
        selected(&executor.docker, ce_labels).await?
    } else {
        Vec::new()
    } {
        let cfg = resource.config.as_ref().ok_or(ExecutionError::Ownership)?;
        let labels = cfg.labels.as_ref().ok_or(ExecutionError::Ownership)?;
        let customer = labels
            .get("webdock.mail.customer")
            .ok_or(ExecutionError::Ownership)?;
        name(customer)?;
        if !incoming.contains(customer.as_str())
            || cfg.user.as_deref() != Some("65532:65532")
            || cfg.env.as_ref().is_some_and(|env| {
                env.iter()
                    .any(|v| v.starts_with("STALWART_RECOVERY_ADMIN="))
            })
        {
            return Err(ExecutionError::Ownership);
        }
        let mount = resource
            .mounts
            .as_ref()
            .and_then(|m| m.iter().find(|m| m.destination.as_deref() == Some("/data")))
            .ok_or(ExecutionError::Ownership)?;
        let volume_name = mount.name.as_deref().ok_or(ExecutionError::Ownership)?;
        let volume = executor
            .docker
            .inspect_volume(volume_name)
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        let path = PathBuf::from(&volume.mountpoint);
        if !path.is_absolute() {
            return Err(ExecutionError::Ownership);
        }
        let bytes = copy_tree(
            &path,
            &ce_root.join(customer),
            &guard,
            c.spec.capacity_bytes,
            0,
        )?;
        ce_receipts.push(json!({"customerID":customer,"image":cfg.image,"sourceRuntimeID":resource.id,"sourceVolume":volume_name,"dataMount":"/data","bytes":bytes,"labels":labels}));
    }
    if ce_receipts.len() != incoming.len() {
        return Err(ExecutionError::Ownership);
    }
    let ledger:Value=serde_json::from_str(&sql(core,"SELECT jsonb_build_object('accepted',(SELECT count(*) FROM webdock_mail.send_job WHERE state='accepted'),'unknown',(SELECT count(*) FROM webdock_mail.send_job WHERE state='unknown'),'queued',(SELECT count(*) FROM webdock_mail.send_job WHERE state='queued'),'epoch',(SELECT epoch::text FROM webdock_auth.native_recovery_epoch WHERE singleton))::text;").await?).map_err(|_|ExecutionError::Runtime)?;
    let material = folder.join("material.json");
    archive::capture_material(profile, BTreeMap::new(), &material)?;
    let envelope = executor.config.custody_directory.join(format!(
        "{}-{}-material.age",
        c.spec.operation_id, c.lease.generation
    ));
    executor
        .guarded(
            c,
            authority,
            cancelled.clone(),
            archive::age(&material, &envelope, profile, false),
        )
        .await?;
    let key_receipt = archive::receipt(&envelope, &profile.key_custody_id)?;
    let archive_path = folder.join("mail.tar");
    archive::pack(&data, &archive_path, &guard, c.spec.capacity_bytes)?;
    let encrypted = folder.join("mail.age");
    executor
        .guarded(
            c,
            authority,
            cancelled.clone(),
            archive::age(&archive_path, &encrypted, profile, false),
        )
        .await?;
    let store = executor.config.store(profile, false).await?;
    let file = tokio::fs::File::open(&encrypted)
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    let object = executor
        .guarded(c, authority, cancelled.clone(), async {
            store
                .upload(
                    &format!("recovery/{}/{}", profile.id, c.spec.policy_id),
                    file,
                    c.spec.capacity_bytes,
                )
                .await
                .map_err(|_| ExecutionError::Runtime)
        })
        .await?;
    authority.checkpoint(c,"checkpoint",json!({"phase":"matched-set-verified","objects":[object],"ledger":ledger,"ce":ce_receipts})).await?;
    // A source with uncertain effects/handoffs remains closed. Clean source
    // compensation is implemented only through the same Core policy/epoch lease.
    let uncertain = sql(metadata, "SELECT count(*) FROM mail_recovery_effect;")
        .await?
        .parse::<u64>()
        .map_err(|_| ExecutionError::Runtime)?;
    let mut resumed = false;
    if uncertain == 0 && ledger["unknown"].as_u64() == Some(0) {
        executor.guard(c, authority).await?;
        authority
            .checkpoint(c, "resume", json!({"phase":"same-fence-compensation"}))
            .await?;
        if sql(metadata,&format!("UPDATE mail_recovery_control SET state='active',revision=revision+1 WHERE singleton AND state='fenced' AND epoch='{}'::uuid AND operation='{}'::uuid AND generation={} AND revision={} RETURNING epoch::text;",private_epoch,c.spec.epoch,private_generation,private_revision)).await? != private_epoch {
            return Err(ExecutionError::Fenced);
        }
        executor.guard(c, authority).await?;
        if metadata_owner != metadata.username && metadata_was_login {
            sql(
                metadata,
                &format!("ALTER ROLE \"{}\" LOGIN;", metadata_owner),
            )
            .await?;
        }
        for participant in runtimes.iter().chain(ce.iter()) {
            executor.guard(c, authority).await?;
            if participant["wantedRunning"] == true {
                let id = participant["id"]
                    .as_str()
                    .ok_or(ExecutionError::Ownership)?;
                executor
                    .docker
                    .start_container(id, None::<bollard::query_parameters::StartContainerOptions>)
                    .await
                    .map_err(|_| ExecutionError::Runtime)?;
            }
        }
        #[cfg(feature = "kubernetes")]
        if let Some((_, client)) = &kube_source {
            for stopped in &kube_stopped {
                executor.guard(c, authority).await?;
                super::kubernetes::resume(client.clone(), stopped, &guard).await?;
            }
        }
        executor.mail_effect_fence.reopen(effect_drain)?;
        resumed = true;
    }
    Ok(
        json!({"version":1,"profile":c.spec.profile,"operationID":c.spec.operation_id,"policyID":c.spec.policy_id,"objects":[object],"keyMaterial":key_receipt,"epoch":c.spec.epoch,"ledger":ledger,"ce":ce_receipts,"metadataOwner":metadata_owner,"sourceResumed":resumed,"sourceRetained":true,"rpo":"canonical/private/CE writers fenced and stopped before matched logical/offline snapshots"}),
    )
}
#[expect(
    clippy::too_many_arguments,
    reason = "The matched Mail restore binds source, target, complete reservation, deadline and live/cold authority explicitly."
)]
pub async fn restore(
    executor: &NativeRecoveryExecutor,
    profile: &RecoveryProfileConfig,
    target: &RecoveryTarget,
    proof: &Value,
    capacity: u64,
    deadline: chrono::DateTime<chrono::Utc>,
    online: Option<(&RecoveryClaim, &dyn RecoveryAuthority)>,
    reservation: &super::RecoveryReservationReceipt,
) -> Result<Value> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let guard = StorageGuard::new(deadline, cancelled.clone());
    let work = restore_inner(
        executor,
        profile,
        target,
        proof,
        capacity,
        deadline,
        online,
        guard,
        reservation,
    );
    if let Some((claim, authority)) = online {
        executor.guarded(claim, authority, cancelled, work).await
    } else {
        tokio::time::timeout(
            (deadline - chrono::Utc::now())
                .to_std()
                .map_err(|_| ExecutionError::Expired)?,
            work,
        )
        .await
        .map_err(|_| ExecutionError::Expired)?
    }
}
struct RestoreScope<'a> {
    executor: &'a NativeRecoveryExecutor,
    online: Option<(&'a RecoveryClaim, &'a dyn RecoveryAuthority)>,
    guard: &'a StorageGuard,
}
impl RestoreScope<'_> {
    async fn check(&self) -> Result<()> {
        super::restore_authority::before_effect(self.online, self.guard).await?;
        self.executor.verify_configuration()
    }
    async fn sql(&self, endpoint: &PostgresEndpoint, statement: &str) -> Result<String> {
        self.check().await?;
        let result = sql(endpoint, statement).await?;
        self.check().await?;
        Ok(result)
    }
    async fn command(
        &self,
        endpoint: &PostgresEndpoint,
        program: &str,
        args: &[&str],
        input: Option<&[u8]>,
    ) -> Result<Vec<u8>> {
        self.check().await?;
        let result = command(endpoint, program, args, input).await?;
        self.check().await?;
        Ok(result)
    }
}
#[expect(
    clippy::too_many_arguments,
    reason = "Private restore body carries the same bound invocation plus its cancellation guard."
)]
async fn restore_inner(
    executor: &NativeRecoveryExecutor,
    profile: &RecoveryProfileConfig,
    target: &RecoveryTarget,
    proof: &Value,
    capacity: u64,
    deadline: chrono::DateTime<chrono::Utc>,
    online: Option<(&RecoveryClaim, &dyn RecoveryAuthority)>,
    guard: StorageGuard,
    reservation: &super::RecoveryReservationReceipt,
) -> Result<Value> {
    let TargetConfiguration::Mail {
        core,
        metadata,
        ce_image,
        volume_plan,
    } = &target.configuration
    else {
        return Err(ExecutionError::Ownership);
    };
    let scope = RestoreScope {
        executor,
        online,
        guard: &guard,
    };
    scope.check().await?;
    for (name, record) in &reservation.volumes {
        let endpoint = match record.role {
            crate::storage::recovery_pool::RecoveryVolumeRole::CoreLedgerPostgres {} => Some(core),
            crate::storage::recovery_pool::RecoveryVolumeRole::PrivateMailPostgres {} => {
                Some(metadata.as_ref())
            }
            crate::storage::recovery_pool::RecoveryVolumeRole::IncomingCe { .. } => None,
        };
        if let Some(endpoint) = endpoint {
            scope.check().await?;
            let installed = executor
                .storage
                .install_recovery_volume(&crate::storage::recovery_pool::RecoveryVolumeRequest {
                    reservation_id: reservation.reservation_id.clone(),
                    grant_sha256: reservation.grant_sha256.clone(),
                    role_name: name.clone(),
                })
                .await?;
            let approved = volume_plan.get(name).ok_or(ExecutionError::Ownership)?;
            scope.check().await?;
            super::docker::start_restored_postgres(
                &executor.docker,
                target,
                endpoint,
                &installed,
                approved,
            )
            .await?;
        }
    }
    let receipt: super::ObjectReceipt = serde_json::from_value(
        proof["objects"]
            .as_array()
            .and_then(|v| v.first())
            .cloned()
            .ok_or(ExecutionError::Ownership)?,
    )
    .map_err(|_| ExecutionError::Ownership)?;
    let key_receipt: archive::CustodyReceipt = serde_json::from_value(proof["keyMaterial"].clone())
        .map_err(|_| ExecutionError::Ownership)?;
    if key_receipt.id != profile.key_custody_id {
        return Err(ExecutionError::Denied);
    }
    let key_path = archive::material_path(&executor.config.custody_directory, &key_receipt)?;
    let folder = reservation.staging_directory.join("mail-restore");
    scope.check().await?;
    directory(&folder)?;
    let material_path = folder.join("material.json");
    scope.check().await?;
    archive::age(&key_path, &material_path, profile, true).await?;
    let material: archive::RecoveryMaterial =
        serde_json::from_slice(&config::private(&material_path, 8 * 1024 * 1024)?)
            .map_err(|_| ExecutionError::Ownership)?;
    let mut recovered = BTreeMap::new();
    for (name, value) in &material.files {
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        {
            return Err(ExecutionError::Ownership);
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(value)
            .map_err(|_| ExecutionError::Ownership)?;
        scope.check().await?;
        let path = folder.join(name);
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .map_err(|_| ExecutionError::Conflict)?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| ExecutionError::Runtime)?;
        recovered.insert(name.clone(), path);
    }
    let encrypted = folder.join("mail.age");
    let store = executor.config.store(profile, true).await?;
    scope.check().await?;
    let mut output = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&encrypted)
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    scope.check().await?;
    store
        .download(
            &format!(
                "recovery/{}/{}",
                profile.id,
                proof["policyID"]
                    .as_str()
                    .ok_or(ExecutionError::Ownership)?
            ),
            &receipt,
            &mut output,
            capacity,
        )
        .await
        .map_err(|_| ExecutionError::Ownership)?;
    output
        .sync_all()
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    drop(output);
    let plain = folder.join("mail.tar");
    scope.check().await?;
    archive::age(&encrypted, &plain, profile, true).await?;
    let data = folder.join("data");
    scope.check().await?;
    directory(&data)?;
    let unpack_guard = guard.clone();
    let unpack_plain = plain.clone();
    let unpack_data = data.clone();
    tokio::task::spawn_blocking(move || {
        archive::unpack(&unpack_plain, &unpack_data, &unpack_guard, capacity)
    })
    .await
    .map_err(|_| ExecutionError::Runtime)??;
    if let Some((claim, authority)) = online {
        executor.guard(claim, authority).await?;
    }
    prepare_fresh_database(&scope, core, &target.nonce).await?;
    prepare_fresh_database(&scope, metadata, &target.nonce).await?;
    // Target roles/schema are provisioned through the existing reviewed migrator.
    let roles = recovered
        .get("database-roles")
        .ok_or(ExecutionError::Denied)?;
    let rls = recovered
        .get("core-rls-secret")
        .ok_or(ExecutionError::Denied)?;
    let rls = String::from_utf8(config::private(rls, 4096)?).map_err(|_| ExecutionError::Denied)?;
    let source = url(core)?;
    let target_binding = format!("{}:{}/{}", core.server_name, core.port, core.database);
    scope.check().await?;
    let result = tokio::process::Command::new("/usr/local/bin/spitzli-migrate")
        .args([
            "--target",
            &target_binding,
            "--initialize-empty",
            "--provision-roles-from",
        ])
        .arg(roles)
        .env("WEBDOCK_MIGRATION_DATABASE_URL", source)
        .env("DATABASE_RLS_SECRET", rls.trim_end())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    if !result.status.success() {
        return Err(ExecutionError::Runtime);
    }
    scope
        .command(
            core,
            "/usr/bin/pg_restore",
            &[
                "--clean",
                "--if-exists",
                "--exit-on-error",
                "--no-password",
                "--dbname",
                &core.database,
                data.join("core.dump")
                    .to_str()
                    .ok_or(ExecutionError::Ownership)?,
            ],
            None,
        )
        .await?;
    let expected_epoch =
        uuid::Uuid::parse_str(proof["epoch"].as_str().ok_or(ExecutionError::Ownership)?)
            .map_err(|_| ExecutionError::Ownership)?
            .to_string();
    let fresh = scope
        .sql(
            core,
            &format!(
                "SELECT webdock_auth.quarantine_fresh_recovery_target('{}'::uuid);",
                expected_epoch
            ),
        )
        .await?;
    if fresh == expected_epoch || uuid::Uuid::parse_str(&fresh).is_err() {
        return Err(ExecutionError::Ownership);
    }
    let owner = name(
        proof["metadataOwner"]
            .as_str()
            .ok_or(ExecutionError::Ownership)?,
    )?;
    if owner != metadata.username {
        scope
            .sql(
                metadata,
                &format!(
                    "CREATE ROLE \"{}\" NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION;",
                    owner
                ),
            )
            .await?;
    }
    scope
        .sql(
            metadata,
            &format!(
                "ALTER DATABASE \"{}\" OWNER TO \"{}\";",
                name(&metadata.database)?,
                owner
            ),
        )
        .await?;
    scope
        .command(
            metadata,
            "/usr/bin/pg_restore",
            &[
                "--exit-on-error",
                "--no-password",
                "--dbname",
                &metadata.database,
                data.join("private.dump")
                    .to_str()
                    .ok_or(ExecutionError::Ownership)?,
            ],
            None,
        )
        .await?;
    scope.sql(metadata,"UPDATE mail_recovery_control SET state='quarantined',epoch=gen_random_uuid(),revision=revision+1 WHERE singleton AND state='fenced';").await?;
    verify_private_key(metadata, &recovered).await?;
    let mut ce_targets = Vec::new();
    for source in proof["ce"].as_array().ok_or(ExecutionError::Ownership)? {
        scope.check().await?;
        let customer = name(
            source["customerID"]
                .as_str()
                .ok_or(ExecutionError::Ownership)?,
        )?;
        if source["image"] != *ce_image {
            return Err(ExecutionError::Ownership);
        }
        let (role_name,role)=reservation.volumes.iter().find(|(_,role)|matches!(&role.role,crate::storage::recovery_pool::RecoveryVolumeRole::IncomingCe{original_customer_id} if original_customer_id.as_str()==customer)).ok_or(ExecutionError::Ownership)?;
        let _ = role;
        let installed = executor
            .storage
            .install_recovery_volume(&crate::storage::recovery_pool::RecoveryVolumeRequest {
                reservation_id: reservation.reservation_id.clone(),
                grant_sha256: reservation.grant_sha256.clone(),
                role_name: role_name.clone(),
            })
            .await?;
        let approved = volume_plan
            .get(role_name)
            .ok_or(ExecutionError::Ownership)?;
        scope.check().await?;
        let runtime = super::docker::start_restored_ce(
            &executor.docker,
            target,
            &executor.pool,
            source["dataMount"]
                .as_str()
                .ok_or(ExecutionError::Ownership)?,
            &installed,
            approved,
        )
        .await?;
        scope.check().await?;
        let gateway = super::docker::start_gateway(
            &executor.docker,
            target,
            &runtime,
            &folder,
            true,
            (deadline - chrono::Utc::now()).num_seconds().max(1) as u64,
        )
        .await?;
        ce_targets.push(json!({"customerID":customer,"runtimeID":runtime,"gatewayRuntimeID":gateway,"networkMode":"none","recoveryMode":true}));
    }
    scope.check().await?;
    Ok(
        json!({"version":1,"dispatch":"quarantined","objects":[receipt],"epoch":fresh,"sourceEpoch":expected_epoch,"ce":ce_targets,"sourceLedger":proof["ledger"],"restoredLedger":serde_json::from_str::<Value>(&scope.sql(core,"SELECT jsonb_build_object('accepted',(SELECT count(*) FROM webdock_mail.send_job WHERE state='accepted'),'unknown',(SELECT count(*) FROM webdock_mail.send_job WHERE state='unknown'),'queued',(SELECT count(*) FROM webdock_mail.send_job WHERE state='queued'))::text;").await?).map_err(|_|ExecutionError::Runtime)?,"targetNonce":target.nonce}),
    )
}
async fn prepare_fresh_database(
    scope: &RestoreScope<'_>,
    endpoint: &PostgresEndpoint,
    nonce: &str,
) -> Result<()> {
    name(&endpoint.database)?;
    let nonce = uuid::Uuid::parse_str(nonce)
        .map_err(|_| ExecutionError::Ownership)?
        .to_string();
    let mut admin = endpoint.clone();
    admin.database = "postgres".into();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        scope.check().await?;
        if scope.sql(&admin, "SELECT 1;").await.is_ok() {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(ExecutionError::Expired);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    if scope
        .sql(
            &admin,
            &format!(
                "SELECT count(*) FROM pg_database WHERE datname='{}';",
                endpoint.database
            ),
        )
        .await?
        != "0"
    {
        return Err(ExecutionError::Ownership);
    }
    scope
        .sql(
            &admin,
            &format!(
                "CREATE DATABASE \"{}\"; COMMENT ON DATABASE \"{}\" IS 'webdock:recovery:{}';",
                endpoint.database, endpoint.database, nonce
            ),
        )
        .await?;
    Ok(())
}
async fn verify_private_key(
    endpoint: &PostgresEndpoint,
    files: &BTreeMap<String, PathBuf>,
) -> Result<()> {
    let key = String::from_utf8(config::private(
        files.get("mail-secret-key").ok_or(ExecutionError::Denied)?,
        4096,
    )?)
    .map_err(|_| ExecutionError::Denied)?;
    let key = spitzli_mail_runtime::secrets::private_payload_key_from_hex(key.trim_end())
        .map_err(|_| ExecutionError::Denied)?;
    let cipher =
        spitzli_mail_runtime::secrets::MailCipher::new(&key).map_err(|_| ExecutionError::Denied)?;
    let sentinel = sql(
        endpoint,
        "SELECT sealed FROM private_payload_key WHERE singleton;",
    )
    .await?;
    if cipher
        .open::<String>(&sentinel, "payload-store")
        .map_err(|_| ExecutionError::Denied)?
        != "webdock-private-payload-v1"
    {
        return Err(ExecutionError::Denied);
    }
    let sample=sql(endpoint,"SELECT jsonb_build_object('customer',customer_id,'reference',reference::text,'sealed',sealed,'bytes',payload_bytes,'sha256',sha256)::text FROM private_payloads ORDER BY reference LIMIT 1;").await?;
    if !sample.is_empty() {
        let sample: Value = serde_json::from_str(&sample).map_err(|_| ExecutionError::Runtime)?;
        let purpose = format!(
            "payload:{}:{}",
            sample["customer"]
                .as_str()
                .ok_or(ExecutionError::Ownership)?,
            sample["reference"]
                .as_str()
                .ok_or(ExecutionError::Ownership)?
        );
        let encoded: String = cipher
            .open(
                sample["sealed"].as_str().ok_or(ExecutionError::Ownership)?,
                &purpose,
            )
            .map_err(|_| ExecutionError::Denied)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| ExecutionError::Denied)?;
        if sample["bytes"].as_u64() != Some(bytes.len() as u64)
            || sample["sha256"] != format!("{:x}", Sha256::digest(&bytes))
        {
            return Err(ExecutionError::Denied);
        }
    }
    Ok(())
}
