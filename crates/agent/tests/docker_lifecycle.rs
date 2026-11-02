#![cfg(feature = "docker")]
mod support;
use bollard::{
    Docker,
    exec::{CreateExecOptions, StartExecResults},
    query_parameters::*,
};
use futures_util::TryStreamExt;
use std::sync::Arc;
use webdock_agent::docker::DockerAdapter;
use webdock_supervisor::*;
struct TestAuthority;
#[async_trait::async_trait]
impl Authorizer for TestAuthority {
    async fn authorize(&self, command: &Command) -> Result<()> {
        if command.spec.installation.starts_with("test-") {
            Ok(())
        } else {
            Err(ExecutionError::Denied)
        }
    }
}
fn command(spec: RuntimeSpec, generation: u64, action: Action) -> Command {
    Command {
        operation_id: format!("test-operation-{generation}"),
        generation,
        lease_until: timestamp().unwrap() + 300,
        authorization: "disposable-test-only".into(),
        material: None,
        action,
        spec,
        expected_runtime_id: None,
    }
}
async fn exec(
    docker: &Docker,
    name: &str,
    args: Vec<&str>,
) -> std::result::Result<String, Box<dyn std::error::Error>> {
    let created = docker
        .create_exec(
            name,
            CreateExecOptions {
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                cmd: Some(args),
                ..Default::default()
            },
        )
        .await?;
    let mut output = String::new();
    if let StartExecResults::Attached {
        output: mut stream, ..
    } = docker.start_exec(&created.id, None).await?
    {
        while let Some(chunk) = stream.try_next().await? {
            output.push_str(&chunk.to_string());
        }
    }
    if docker.inspect_exec(&created.id).await?.exit_code != Some(0) {
        return Err("container command failed".into());
    }
    Ok(output)
}
#[tokio::test]
#[ignore = "requires explicit WEBDOCK_DOCKER_TEST_IMAGE digest and local Engine"]
async fn isolated_docker_lifecycle_preserves_data_and_reconciles_without_duplicate_containers() {
    let image = std::env::var("WEBDOCK_DOCKER_TEST_IMAGE")
        .expect("set a reviewed public test image digest");
    let installation = format!("test-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let mut spec = support::spec();
    spec.installation = installation.clone();
    spec.image = image.clone();
    spec.volume = Some(RetainedVolume {
        mount_path: "/data".into(),
        size_bytes: None,
    });
    spec.arguments.extend(["-h".into(), "/data".into()]);
    let docker =
        Docker::connect_with_unix("/var/run/docker.sock", 10, bollard::API_DEFAULT_VERSION)
            .unwrap()
            .negotiate_version()
            .await
            .unwrap();
    let adapter = DockerAdapter::connect("/var/run/docker.sock", installation, Some(image))
        .await
        .unwrap();
    let name = spec.name();
    let network = format!("{name}-net");
    let volume = format!("{name}-data");
    let result: std::result::Result<(), Box<dyn std::error::Error>> = async {
        let c = command(spec.clone(), 1, Action::Apply);
        let guard = LeaseGuard::new(c.clone(), Arc::new(TestAuthority));
        let first = adapter.reconcile(&c, &guard).await?;
        assert_eq!(first.phase, Phase::Running);
        exec(
            &docker,
            &name,
            vec!["/bin/sh", "-c", "printf retained-data > /data/index.html"],
        )
        .await?;
        let retry = adapter.reconcile(&c, &guard).await?;
        assert_eq!(first.runtime_id, retry.runtime_id);
        assert!(
            retry.ready,
            "owned HTTP health probe must pass after writing the index"
        );
        assert_eq!(
            exec(&docker, &name, vec!["cat", "/data/index.html"]).await?,
            "retained-data"
        );
        let stop = command(
            RuntimeSpec {
                revision: 2,
                ..spec.clone()
            },
            2,
            Action::Stop,
        );
        let stopped = adapter
            .reconcile(
                &stop,
                &LeaseGuard::new(stop.clone(), Arc::new(TestAuthority)),
            )
            .await?;
        assert_eq!(stopped.phase, Phase::Stopped);
        assert_eq!(stopped.revision, 2);
        assert_eq!(adapter.inspect(&spec).await?.revision, 1);
        use std::os::unix::fs::PermissionsExt;
        let stopped_directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()?;
        let mut journal = Journal::open(stopped_directory.path().join("journal.json"))?;
        journal.claim(&stop)?;
        journal.complete(&stop, &stopped)?;
        let stopped_executor =
            Executor::new(journal, Arc::new(TestAuthority), Arc::new(adapter.clone()));
        let mut renewed_stop = stop.clone();
        renewed_stop.generation += 1;
        let verified_stop = stopped_executor.execute(renewed_stop.clone()).await?;
        assert_eq!(verified_stop.phase, Phase::Stopped);
        assert_eq!(verified_stop.revision, 2);
        assert_eq!(verified_stop.runtime_id, stopped.runtime_id);
        assert_eq!(adapter.inspect(&spec).await?.revision, 1);
        let stopped_id = stopped.runtime_id.as_deref().unwrap();
        docker
            .start_container(stopped_id, None::<StartContainerOptions>)
            .await?;
        renewed_stop.generation += 1;
        assert!(matches!(
            stopped_executor.execute(renewed_stop).await,
            Err(ExecutionError::Runtime)
        ));
        assert_eq!(
            docker
                .inspect_container(stopped_id, None::<InspectContainerOptions>)
                .await?
                .state
                .unwrap()
                .running,
            Some(true)
        );
        docker
            .stop_container(
                stopped_id,
                Some(StopContainerOptionsBuilder::default().t(1).build()),
            )
            .await?;
        docker.inspect_volume(&volume).await?;
        spec.revision = 3;
        let resume = command(spec.clone(), 3, Action::Apply);
        let running = adapter
            .reconcile(
                &resume,
                &LeaseGuard::new(resume.clone(), Arc::new(TestAuthority)),
            )
            .await?;
        assert_eq!(running.phase, Phase::Running);
        assert_eq!(
            exec(&docker, &name, vec!["cat", "/data/index.html"]).await?,
            "retained-data"
        );
        spec.revision = 4;
        spec.memory_bytes = 64 * 1024 * 1024;
        let mut upgrade = command(spec.clone(), 4, Action::Apply);
        upgrade.expected_runtime_id = running.runtime_id.clone();
        let mut wrong = upgrade.clone();
        wrong.expected_runtime_id = Some("wrong-runtime".into());
        assert!(matches!(
            adapter
                .reconcile(
                    &wrong,
                    &LeaseGuard::new(wrong.clone(), Arc::new(TestAuthority))
                )
                .await,
            Err(ExecutionError::Ownership)
        ));
        let changed = adapter
            .reconcile(
                &upgrade,
                &LeaseGuard::new(upgrade.clone(), Arc::new(TestAuthority)),
            )
            .await?;
        assert_ne!(changed.runtime_id, running.runtime_id);
        let replay = adapter
            .reconcile(
                &upgrade,
                &LeaseGuard::new(upgrade.clone(), Arc::new(TestAuthority)),
            )
            .await?;
        assert_eq!(replay.runtime_id, changed.runtime_id);
        let mut renewed = upgrade.clone();
        renewed.generation = 5;
        assert_eq!(
            adapter
                .reconcile(
                    &renewed,
                    &LeaseGuard::new(renewed.clone(), Arc::new(TestAuthority))
                )
                .await?
                .runtime_id,
            changed.runtime_id
        );
        let effective = docker
            .inspect_container(&name, None::<InspectContainerOptions>)
            .await?;
        assert_eq!(
            effective.host_config.unwrap().memory,
            Some(64 * 1024 * 1024)
        );
        assert_eq!(
            exec(&docker, &name, vec!["cat", "/data/index.html"]).await?,
            "retained-data"
        );
        assert!(adapter.logs(&spec, 10).await?.len() <= 65536);
        let metrics = adapter.metrics(&spec).await?;
        assert!(metrics.cpu_nanoseconds.is_some());
        assert!(metrics.memory_bytes.is_some());
        let mut foreign = spec.clone();
        foreign.project_id = webdock_core::id::RecordId::parse("999").unwrap();
        assert!(matches!(
            adapter.inspect(&foreign).await,
            Err(ExecutionError::Ownership)
        ));
        let journal_directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()?;
        let executor = Executor::new(
            Journal::open(journal_directory.path().join("journal.json"))?,
            Arc::new(TestAuthority),
            Arc::new(adapter.clone()),
        );
        spec.revision = 5;
        let mut retire = command(spec.clone(), 6, Action::Retire);
        retire.expected_runtime_id = changed.runtime_id;
        let retired = executor.execute(retire.clone()).await?;
        assert!(retired.ready);
        retire.generation = 7;
        let verified = executor.execute(retire.clone()).await?;
        assert_eq!(verified.phase, Phase::Absent);
        assert_eq!(verified.storage, retired.storage);
        docker.inspect_volume(&volume).await?;
        spec.revision = 6;
        let mut purge = command(spec.clone(), 8, Action::PurgeStorage);
        let purged = executor.execute(purge.clone()).await?;
        assert_eq!(
            purged.storage,
            Some(StorageObservation {
                volume_bytes: Some(0),
                retained: false
            })
        );
        purge.generation = 9;
        assert_eq!(
            executor.execute(purge.clone()).await?.storage,
            purged.storage
        );
        docker
            .create_volume(bollard::models::VolumeCreateRequest {
                name: Some(volume.clone()),
                labels: Some(std::collections::HashMap::from([(
                    "fixture".into(),
                    "foreign-replacement".into(),
                )])),
                ..Default::default()
            })
            .await?;
        purge.generation = 10;
        assert!(matches!(
            executor.execute(purge.clone()).await,
            Err(ExecutionError::Ownership)
        ));
        assert_eq!(
            docker
                .inspect_volume(&volume)
                .await?
                .labels
                .get("fixture")
                .map(String::as_str),
            Some("foreign-replacement")
        );
        drop(executor);
        let executor = Executor::new(
            Journal::open(journal_directory.path().join("journal.json"))?,
            Arc::new(TestAuthority),
            Arc::new(adapter.clone()),
        );
        assert!(matches!(
            executor.execute(purge.clone()).await,
            Err(ExecutionError::Ownership)
        ));
        docker
            .remove_volume(&volume, None::<RemoveVolumeOptions>)
            .await?;
        assert_eq!(executor.execute(purge).await?.storage, purged.storage);
        Ok(())
    }
    .await;
    // Only names derived from this test's random installation are removed; no daemon-wide pruning.
    for container in [name.clone(), format!("{name}-init")] {
        let _ = docker
            .remove_container(
                &container,
                Some(
                    RemoveContainerOptionsBuilder::default()
                        .force(true)
                        .v(false)
                        .build(),
                ),
            )
            .await;
    }
    let _ = docker
        .remove_volume(&volume, None::<RemoveVolumeOptions>)
        .await;
    let _ = docker.remove_network(&network).await;
    result.unwrap();
}

#[tokio::test]
#[ignore = "requires explicit disposable WEBDOCK_DOCKER_TEST_IMAGE and local Engine"]
async fn scoped_secret_and_socket_mounts_use_reviewed_descriptor_and_nonroot_identity() {
    use sha2::{Digest, Sha256};
    use std::{io::Write, os::unix::fs::PermissionsExt};
    use webdock_agent::policy::{GrantKind, MountGrant, RuntimePolicy, RuntimePolicyConfig};
    let image = std::env::var("WEBDOCK_DOCKER_TEST_IMAGE").unwrap();
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let mut spec = support::spec();
    spec.image = image;
    spec.installation = format!("test-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    spec.health_path = None;
    let uid = rustix::process::geteuid().as_raw();
    assert_ne!(
        uid, 0,
        "run the local socket fixture as the non-root runtime owner"
    );
    let artifacts = directory.path().join("artifacts");
    std::fs::create_dir(&artifacts).unwrap();
    std::fs::set_permissions(&artifacts, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut manifest: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../plugin-protocol/tests/fixtures/project-notes.json"
    ))
    .unwrap();
    manifest["name"] = "socket-fixture".into();
    manifest["image"] = spec.image.clone().into();
    manifest["jobs"] = serde_json::json!([]);
    manifest["secrets"] = serde_json::json!(["token"]);
    manifest["capabilities"]
        .as_array_mut()
        .unwrap()
        .push("plugin.secrets.read".into());
    let bytes = serde_json::to_vec(&manifest).unwrap();
    let manifest_hash = format!("{:x}", Sha256::digest(&bytes));
    let mut file = tempfile::NamedTempFile::new_in(&artifacts).unwrap();
    file.write_all(&bytes).unwrap();
    file.persist(artifacts.join(format!("{manifest_hash}.json")))
        .unwrap();
    let root = directory.path().join("mounts");
    let resource = root.join(&spec.installation).join("123/456");
    let secrets = resource.join("secrets");
    let sockets = resource.join("sockets/rpc");
    std::fs::create_dir_all(&secrets).unwrap();
    std::fs::create_dir_all(&sockets).unwrap();
    for path in [
        &root,
        &root.join(&spec.installation),
        &root.join(&spec.installation).join("123"),
        &resource,
        &secrets,
        &resource.join("sockets"),
        &sockets,
    ] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut file = tempfile::NamedTempFile::new_in(&secrets).unwrap();
    file.write_all(b"scoped-test-only").unwrap();
    file.persist(secrets.join("token")).unwrap();
    let socket = sockets.join("core.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let grant = |name: &str, kind| MountGrant {
        name: name.into(),
        revision: 1,
        installation: spec.installation.clone(),
        project_id: "123".into(),
        resource_id: "456".into(),
        kind,
    };
    let policy = RuntimePolicy::load(RuntimePolicyConfig {
        registry_config: None,
        artifact_directory: Some(artifacts),
        artifacts: vec![manifest_hash.clone()],
        mount_root: Some(root),
        runtime_uid: Some(uid),
        grants: vec![
            grant(
                "token",
                GrantKind::Secret {
                    sha256: format!("{:x}", Sha256::digest(b"scoped-test-only")),
                },
            ),
            grant("rpc", GrantKind::SocketDirectory {}),
        ],
    })
    .unwrap();
    spec.plugin = Some(PluginBinding {
        manifest_sha256: manifest_hash.clone(),
        plugin_id: "socket-fixture".into(),
        version: "1.0.0".into(),
        runtime_generation: 7,
    });
    spec.mounts = vec![
        MountReference {
            name: "token".into(),
            revision: 1,
        },
        MountReference {
            name: "rpc".into(),
            revision: 1,
        },
    ];
    let adapter = DockerAdapter::connect("/var/run/docker.sock", spec.installation.clone(), None)
        .await
        .unwrap()
        .with_policy(Arc::new(policy));
    let docker =
        Docker::connect_with_unix("/var/run/docker.sock", 10, bollard::API_DEFAULT_VERSION)
            .unwrap()
            .negotiate_version()
            .await
            .unwrap();
    let name = spec.name();
    let task_docker = docker.clone();
    let task_name = name.clone();
    let result = tokio::spawn(async move {
        let c = command(spec, 1, Action::Apply);
        adapter
            .reconcile(&c, &LeaseGuard::new(c.clone(), Arc::new(TestAuthority)))
            .await
            .unwrap();
        assert_eq!(
            exec(&task_docker, &task_name, vec!["cat", "/run/secrets/token"])
                .await
                .unwrap(),
            "scoped-test-only"
        );
        exec(
            &task_docker,
            &task_name,
            vec!["test", "-S", "/run/webdock/rpc/core.sock"],
        )
        .await
        .unwrap();
        assert!(
            exec(
                &task_docker,
                &task_name,
                vec!["/bin/sh", "-c", "printf forbidden > /run/secrets/token"]
            )
            .await
            .is_err()
        );
        let config = task_docker
            .inspect_container(&task_name, None::<InspectContainerOptions>)
            .await
            .unwrap()
            .config
            .unwrap();
        assert_eq!(config.user, Some(format!("{uid}:{uid}")));
        assert!(
            config
                .env
                .unwrap()
                .contains(&format!("WEBDOCK_PLUGIN_MANIFEST_SHA256={manifest_hash}"))
        );
        let revoked =
            DockerAdapter::connect("/var/run/docker.sock", c.spec.installation.clone(), None)
                .await
                .unwrap();
        assert!(
            revoked
                .reconcile(&c, &LeaseGuard::new(c.clone(), Arc::new(TestAuthority)))
                .await
                .is_err()
        );
        let mut stop = command(c.spec.clone(), 2, Action::Stop);
        stop.expected_runtime_id = adapter.inspect(&c.spec).await.unwrap().runtime_id;
        assert_eq!(
            revoked
                .reconcile(
                    &stop,
                    &LeaseGuard::new(stop.clone(), Arc::new(TestAuthority))
                )
                .await
                .unwrap()
                .phase,
            Phase::Stopped
        );
        let mut retire = command(c.spec.clone(), 3, Action::Retire);
        retire.expected_runtime_id = stop.expected_runtime_id;
        assert_eq!(
            revoked
                .reconcile(
                    &retire,
                    &LeaseGuard::new(retire.clone(), Arc::new(TestAuthority))
                )
                .await
                .unwrap()
                .phase,
            Phase::Absent
        );
    })
    .await;
    let _ = docker
        .remove_container(
            &name,
            Some(
                RemoveContainerOptionsBuilder::default()
                    .force(true)
                    .v(false)
                    .build(),
            ),
        )
        .await;
    let _ = docker.remove_network(&format!("{name}-net")).await;
    drop(listener);
    assert_eq!(
        std::fs::read(secrets.join("token")).unwrap(),
        b"scoped-test-only"
    );
    result.unwrap();
}

#[tokio::test]
#[ignore = "requires explicit owned local Docker fixture image"]
async fn hosted_environment_rotation_is_scoped_ephemeral_and_can_clear_values() {
    let image = std::env::var("WEBDOCK_DOCKER_TEST_IMAGE").unwrap();
    let installation = format!("test-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let mut spec = support::spec();
    spec.installation = installation.clone();
    spec.image = image;
    spec.arguments = vec!["sleep".into(), "300".into()];
    spec.health_path = None;
    let engine =
        Docker::connect_with_unix("/var/run/docker.sock", 10, bollard::API_DEFAULT_VERSION)
            .unwrap()
            .negotiate_version()
            .await
            .unwrap();
    let adapter = DockerAdapter::connect("/var/run/docker.sock", installation, None)
        .await
        .unwrap();
    let name = spec.name();
    let test_name = name.clone();
    let test_engine = engine.clone();
    let result = tokio::spawn(async move {
        let mut expected = None;
        for (revision, value, replicas) in [
            (1, Some("fixture-first"), 0),
            (2, Some("fixture-second"), 0),
            (3, Some("fixture-third"), 1),
            (4, None, 1),
        ] {
            spec.revision = revision;
            spec.replicas = replicas;
            spec.hosted_access = Some(HostedAccess {
                service_bindings: None,
                environment: Some(EnvironmentReference {
                    revision,
                    source_sha256: format!("{revision:064x}"),
                }),
            });
            let mut c = command(spec.clone(), revision, Action::Apply);
            c.expected_runtime_id = expected;
            c.material = Some(RuntimeMaterial {
                service_bindings: None,
                operation_id: c.operation_id.clone(),
                request_sha256: c.fingerprint().unwrap(),
                environment: Some(EnvironmentMaterial {
                    source_sha256: format!("{revision:064x}"),
                    values: value
                        .map(|v| {
                            std::collections::BTreeMap::from([("FIXTURE_TOKEN".into(), v.into())])
                        })
                        .unwrap_or_default(),
                }),
            });
            let observed = adapter
                .reconcile(&c, &LeaseGuard::new(c.clone(), Arc::new(TestAuthority)))
                .await
                .unwrap();
            expected = observed.runtime_id;
            if replicas == 0 {
                assert_eq!(observed.phase, Phase::Stopped);
                let actual = test_engine
                    .inspect_container(&test_name, None::<InspectContainerOptions>)
                    .await
                    .unwrap();
                assert_eq!(actual.state.unwrap().running, Some(false));
                assert!(
                    actual
                        .config
                        .unwrap()
                        .env
                        .unwrap()
                        .contains(&format!("FIXTURE_TOKEN={}", value.unwrap()))
                );
            } else {
                let output = exec(
                    &test_engine,
                    &test_name,
                    vec!["sh", "-c", "printf %s \"${FIXTURE_TOKEN-unset}\""],
                )
                .await
                .unwrap();
                assert_eq!(output, value.unwrap_or("unset"));
            }
            assert!(!serde_json::to_string(&c).unwrap().contains("fixture-first"));
        }
        spec.revision = 5;
        spec.hosted_access
            .as_mut()
            .unwrap()
            .environment
            .as_mut()
            .unwrap()
            .revision = 5;
        let mut missing = command(spec, 5, Action::Apply);
        missing.expected_runtime_id = expected.clone();
        assert!(
            adapter
                .reconcile(
                    &missing,
                    &LeaseGuard::new(missing.clone(), Arc::new(TestAuthority))
                )
                .await
                .is_err()
        );
        assert_eq!(
            adapter.inspect(&missing.spec).await.unwrap().runtime_id,
            expected
        );
    })
    .await;
    let _ = engine
        .remove_container(
            &name,
            Some(
                RemoveContainerOptionsBuilder::default()
                    .force(true)
                    .v(false)
                    .build(),
            ),
        )
        .await;
    let _ = engine.remove_network(&format!("{name}-net")).await;
    result.unwrap();
}
