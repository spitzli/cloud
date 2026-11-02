use webdock_agent::storage::{StorageConfig, StoragePacket, VolumeRecord, VolumeState};
#[test]
fn legacy_pool_and_volume_wire_fields_round_trip_without_identity_changes() {
    let config = serde_json::json!({"root":"/var/lib/webdock-storage","node":"node-one","clusterID":"789","poolID":"123e4567-e89b-42d3-a456-426614174000","capacityBytes":1073741824u64,"reserveBytes":67108864u64});
    let parsed: StorageConfig = serde_json::from_value(config.clone()).unwrap();
    parsed.validate().unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), config);
    let record:VolumeRecord=serde_json::from_value(serde_json::json!({"appID":"456","projectID":"123","clusterID":"789","bytes":67108864u64,"uuid":"123e4567-e89b-42d3-a456-426614174000","state":"ready"})).unwrap();
    let packet = StoragePacket::new(
        "456",
        "123",
        "789",
        1,
        67108864,
        webdock_supervisor::timestamp().unwrap() + 60,
    )
    .unwrap();
    record.authorize(&packet).unwrap();
    assert_eq!(record.state, VolumeState::Ready);
    let resize = StoragePacket::new(
        "456",
        "123",
        "789",
        2,
        134217728,
        webdock_supervisor::timestamp().unwrap() + 60,
    )
    .unwrap();
    assert!(record.authorize(&resize).is_err());
    let other = StoragePacket::new(
        "456",
        "999",
        "789",
        1,
        67108864,
        webdock_supervisor::timestamp().unwrap() + 60,
    )
    .unwrap();
    assert!(record.authorize(&other).is_err());
}
#[test]
fn storage_packets_reject_traversal_expired_leases_and_unknown_parameters() {
    assert!(
        StoragePacket::new(
            "../456",
            "123",
            "789",
            1,
            67108864,
            webdock_supervisor::timestamp().unwrap() + 60
        )
        .is_err()
    );
    assert!(StoragePacket::new("456", "123", "789", 1, 67108864, 0).is_err());
    let mut value = serde_json::to_value(
        StoragePacket::new(
            "456",
            "123",
            "789",
            1,
            67108864,
            webdock_supervisor::timestamp().unwrap() + 60,
        )
        .unwrap(),
    )
    .unwrap();
    value["path"] = "/etc".into();
    assert!(serde_json::from_value::<StoragePacket>(value).is_err());
}

#[tokio::test]
#[ignore = "requires explicitly owned privileged disposable container; no host mounts"]
async fn disposable_filesystem_preserves_identity_enforces_bytes_and_serves_private_rpc() {
    use std::{io::Write, os::unix::fs::PermissionsExt};
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();
    use webdock_agent::storage::{StorageClient, StoragePool};
    assert_eq!(
        std::env::var("WEBDOCK_DISPOSABLE_STORAGE_TEST").as_deref(),
        Ok("yes")
    );
    assert!(std::path::Path::new("/.dockerenv").is_file());
    assert_eq!(rustix::process::geteuid().as_raw(), 0);
    let base = tempfile::Builder::new()
        .prefix("webdock-storage-owned-")
        .tempdir()
        .unwrap();
    std::fs::set_permissions(base.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = base.path().join("volumes");
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let config_path = base.path().join("config.json");
    let config = serde_json::json!({"root":root,"node":"fixture-node","clusterID":"789","poolID":uuid::Uuid::new_v4().to_string(),"capacityBytes":268435456,"reserveBytes":67108864});
    std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    StoragePool::initialize(&config_path).unwrap();
    let socket = base.path().join("helper.sock");
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let serve_config = config_path.clone();
    let serve_socket = socket.clone();
    let server = tokio::spawn(async move {
        webdock_agent::storage::serve(&serve_config, &serve_socket, async {
            let _ = stop_rx.await;
        })
        .await
    });
    let test_root = root.clone();
    let test_socket = socket.clone();
    let test=tokio::spawn(async move {
        for _ in 0..200 {if test_socket.exists(){break}tokio::time::sleep(std::time::Duration::from_millis(10)).await;}
        let client=StorageClient::new(test_socket);assert_eq!(client.information().await.unwrap().root,test_root);
        let nodes=vec!["fixture-node".to_string()];assert!(client.capability(&nodes).await.unwrap().ready);
        let mut packet=StoragePacket::new("456","123","789",1,67108864,webdock_supervisor::timestamp().unwrap()+120).unwrap();
        let (_,path)=client.ensure(&packet).await.unwrap();let record_path=test_root.join("_pool/state.json");let initial:serde_json::Value=serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
        std::fs::write(path.join("proof"),b"retained-content").unwrap();client.retained(&packet).await.unwrap();
        assert_eq!(client.volume_state(&packet).await.unwrap(), VolumeState::Ready);
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&std::fs::read(&record_path).unwrap()).unwrap(),initial);
        let mut file=std::fs::File::create(path.join("fill")).unwrap();let block=vec![0u8;1024*1024];let mut written=0u64;
        loop {match file.write_all(&block){Ok(())=>{written+=block.len() as u64;assert!(written<packet.spec.volume_bytes,"filesystem exceeded approved allocation");},Err(error)=>{assert_eq!(error.raw_os_error(),Some(28));break;}}}
        assert!(written>0);drop(file);std::fs::remove_file(path.join("fill")).unwrap();
        let wrong=StoragePacket::new("456","999","789",1,67108864,webdock_supervisor::timestamp().unwrap()+120).unwrap();assert!(client.ensure(&wrong).await.is_err());
        let resize=StoragePacket::new("456","123","789",2,134217728,webdock_supervisor::timestamp().unwrap()+120).unwrap();assert!(client.ensure(&resize).await.is_err());
        assert!(std::process::Command::new("/usr/bin/umount").arg(test_root.join("456/mount")).status().unwrap().success());assert!(!path.exists());assert!(!client.capability(&nodes).await.unwrap().ready);
        assert!(client.volume_state(&packet).await.is_err());assert!(!path.exists(),"inspection must not remount missing data");
        let image=test_root.join("456/volume.ext4");let absent=test_root.join("456/volume.missing");std::fs::rename(&image,&absent).unwrap();packet.revision=2;assert!(client.ensure(&packet).await.is_err());assert!(!image.exists());std::fs::rename(&absent,&image).unwrap();
        let (_,restored)=client.ensure(&packet).await.unwrap();assert_eq!(restored,path);assert_eq!(std::fs::read(path.join("proof")).unwrap(),b"retained-content");assert!(client.capability(&nodes).await.unwrap().ready);
        let after:serde_json::Value=serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();assert_eq!(initial,after);
        client.purge(&packet).await.unwrap();client.purge(&packet).await.unwrap();assert!(!image.exists());assert!(client.ensure(&packet).await.is_err());
        assert_eq!(client.volume_state(&packet).await.unwrap(),VolumeState::Purged);
        assert!(client.volume_state(&wrong).await.is_err());
        let missing=StoragePacket::new("457","123","789",1,67108864,webdock_supervisor::timestamp().unwrap()+120).unwrap();assert!(client.volume_state(&missing).await.is_err());
        std::fs::write(&image,b"foreign replacement").unwrap();assert!(client.volume_state(&packet).await.is_err());assert_eq!(std::fs::read(&image).unwrap(),b"foreign replacement");std::fs::remove_file(&image).unwrap();
        eprintln!("owned fixed filesystem: written_before_enospc={written}; retained identity/content; missing image/ownership/resize denied; purge tombstone preserved");
    }).await;
    let _ = stop_tx.send(());
    let server_result = server.await;
    // Cleanup is limited to this fixture's exact image and mount. Never detach unrelated loops.
    let image = root.join("456/volume.ext4");
    let mount = root.join("456/mount");
    if mount.exists() {
        let _ = std::process::Command::new("/usr/bin/umount")
            .arg(&mount)
            .output();
    }
    if image.exists() {
        let loops = std::process::Command::new("/usr/sbin/losetup")
            .args(["--json", "--list", "--associated"])
            .arg(&image)
            .output()
            .unwrap();
        assert!(loops.status.success());
        let loops: serde_json::Value = serde_json::from_slice(&loops.stdout).unwrap();
        for entry in loops["loopdevices"].as_array().unwrap() {
            assert_eq!(entry["back-file"], image.to_str().unwrap());
            let device = entry["name"].as_str().unwrap();
            assert!(
                device
                    .strip_prefix("/dev/loop")
                    .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            );
            assert!(
                std::process::Command::new("/usr/sbin/losetup")
                    .args(["--detach", device])
                    .status()
                    .unwrap()
                    .success()
            );
        }
    }
    server_result.unwrap().unwrap();
    test.unwrap();
}

#[cfg(feature = "docker")]
#[test]
fn docker_quota_binding_requires_exact_pool_identity_driver_and_derived_source() {
    use std::collections::HashMap;
    let config:StorageConfig=serde_json::from_value(serde_json::json!({"root":"/var/lib/fixture-storage","node":"fixture-node","clusterID":"789","poolID":"123e4567-e89b-42d3-a456-426614174000","capacityBytes":268435456,"reserveBytes":67108864})).unwrap();
    let mut spec = support_spec();
    spec.volume = Some(webdock_supervisor::RetainedVolume {
        mount_path: "/data".into(),
        size_bytes: Some(67108864),
    });
    let mut labels: HashMap<_, _> = spec.labels().into_iter().collect();
    labels.insert("webdock.dev/storage-pool".into(), config.pool_id.clone());
    labels.insert("webdock.dev/storage-bytes".into(), "67108864".into());
    let mut volume = bollard::models::Volume {
        name: format!("{}-data", spec.name()),
        driver: "local".into(),
        labels,
        options: HashMap::from([
            ("type".into(), "none".into()),
            ("o".into(), "bind".into()),
            (
                "device".into(),
                "/var/lib/fixture-storage/456/mount/data".into(),
            ),
        ]),
        ..Default::default()
    };
    webdock_agent::docker::verify_quota_volume(&volume, &spec, &config).unwrap();
    volume.options.insert("device".into(), "/etc".into());
    assert!(webdock_agent::docker::verify_quota_volume(&volume, &spec, &config).is_err());
    volume.options.insert(
        "device".into(),
        "/var/lib/fixture-storage/456/mount/data".into(),
    );
    volume.labels.insert(
        "webdock.dev/storage-pool".into(),
        uuid::Uuid::new_v4().to_string(),
    );
    assert!(webdock_agent::docker::verify_quota_volume(&volume, &spec, &config).is_err());
}
#[cfg(feature = "docker")]
fn support_spec() -> webdock_supervisor::RuntimeSpec {
    serde_json::from_value(serde_json::json!({"version":1,"installation":"test-quota","projectId":"123","resourceId":"456","revision":1,"image":format!("docker.io/library/busybox@sha256:{}","a".repeat(64)),"arguments":["sleep","300"],"cpuMillicores":100,"memoryBytes":33554432,"ephemeralBytes":16777216,"replicas":1,"port":8080,"healthPath":null,"volume":null})).unwrap()
}

#[cfg(feature = "docker")]
#[tokio::test]
#[ignore = "requires explicitly owned privileged container and nested Engine; no host mounts/socket"]
async fn disposable_docker_volume_has_real_byte_limit_and_retains_data() {
    use bollard::{
        Docker,
        exec::{CreateExecOptions, StartExecResults},
        query_parameters::*,
    };
    use futures_util::TryStreamExt;
    use std::{os::unix::fs::PermissionsExt, sync::Arc};
    use webdock_agent::{
        docker::DockerAdapter,
        storage::{StorageClient, StoragePool},
    };
    use webdock_supervisor::*;
    assert_eq!(
        std::env::var("WEBDOCK_DISPOSABLE_STORAGE_TEST").as_deref(),
        Ok("yes")
    );
    assert!(std::path::Path::new("/.dockerenv").is_file());
    assert_eq!(rustix::process::geteuid().as_raw(), 0);
    let socket = "/run/webdock-owned-docker.sock";
    let docker = Docker::connect_with_unix(socket, 10, bollard::API_DEFAULT_VERSION)
        .unwrap()
        .negotiate_version()
        .await
        .unwrap();
    let info = docker.info().await.unwrap();
    let node = info.name.unwrap();
    let base = tempfile::Builder::new()
        .prefix("webdock-quota-owned-")
        .tempdir()
        .unwrap();
    std::fs::set_permissions(base.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = base.path().join("volumes");
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let config_path = base.path().join("config.json");
    let config = serde_json::json!({"root":root,"node":node,"clusterID":"789","poolID":uuid::Uuid::new_v4().to_string(),"capacityBytes":268435456,"reserveBytes":67108864});
    std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    StoragePool::initialize(&config_path).unwrap();
    let helper_socket = base.path().join("storage.sock");
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let server_socket = helper_socket.clone();
    let server = tokio::spawn(async move {
        webdock_agent::storage::serve(&config_path, &server_socket, async {
            let _ = stop_rx.await;
        })
        .await
    });
    for _ in 0..200 {
        if helper_socket.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let mut spec = support_spec();
    spec.image="docker.io/library/busybox@sha256:bdf57e528e45e4433820e045b29b4597825a1c9e38353532d90a01445013f82e".into();
    spec.memory_bytes = 134217728;
    spec.volume = Some(RetainedVolume {
        mount_path: "/data".into(),
        size_bytes: Some(67108864),
    });
    struct Authority;
    #[async_trait::async_trait]
    impl Authorizer for Authority {
        async fn authorize(&self, c: &Command) -> Result<()> {
            if c.spec.installation == "test-quota" {
                Ok(())
            } else {
                Err(ExecutionError::Denied)
            }
        }
    }
    let mut command = Command {
        operation_id: "quota-create".into(),
        generation: 1,
        lease_until: timestamp().unwrap() + 300,
        authorization: "fixture-only".into(),
        material: None,
        action: Action::Apply,
        spec: spec.clone(),
        expected_runtime_id: None,
    };
    async fn exec(docker: &Docker, name: &str, script: &str) -> (i64, String) {
        let created = docker
            .create_exec(
                name,
                CreateExecOptions {
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    cmd: Some(vec!["/bin/sh", "-c", script]),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let mut text = String::new();
        if let StartExecResults::Attached {
            output: mut stream, ..
        } = docker.start_exec(&created.id, None).await.unwrap()
        {
            while let Some(chunk) = stream.try_next().await.unwrap() {
                text.push_str(&chunk.to_string());
            }
        }
        (
            docker
                .inspect_exec(&created.id)
                .await
                .unwrap()
                .exit_code
                .unwrap(),
            text,
        )
    }
    let docker_test = docker.clone();
    let helper_test = helper_socket.clone();
    let mut test_command = command.clone();
    let test = tokio::spawn(async move {
        let unavailable = DockerAdapter::connect(socket, "test-quota".into(), None)
            .await
            .unwrap();
        assert!(!unavailable.capabilities().volume_quotas);
        assert!(
            unavailable
                .reconcile(
                    &test_command,
                    &LeaseGuard::new(test_command.clone(), Arc::new(Authority))
                )
                .await
                .is_err()
        );
        let adapter = unavailable
            .with_storage_socket(helper_test.clone())
            .await
            .unwrap();
        assert!(adapter.capabilities().volume_quotas);
        let snapshot = adapter.target_observation().await.unwrap().unwrap();
        assert!(snapshot.environment);
        assert_eq!(
            snapshot.storage,
            Some(StorageCapacity {
                capacity_bytes: 268435456,
                ready: true
            })
        );
        let config_path = helper_test.parent().unwrap().join("config.json");
        std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o640)).unwrap();
        let degraded = adapter.target_observation().await.unwrap().unwrap();
        assert_eq!(
            degraded.storage,
            Some(StorageCapacity {
                capacity_bytes: 0,
                ready: false
            })
        );
        std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let first = adapter
            .reconcile(
                &test_command,
                &LeaseGuard::new(test_command.clone(), Arc::new(Authority)),
            )
            .await
            .unwrap();
        assert_eq!(first.phase, Phase::Running);
        assert_eq!(
            first.storage,
            Some(StorageObservation {
                volume_bytes: Some(67108864),
                retained: false
            })
        );
        let name = test_command.spec.name();
        assert_eq!(
            exec(&docker_test, &name, "printf retained-quota > /data/proof")
                .await
                .0,
            0
        );
        let fill = exec(
            &docker_test,
            &name,
            "dd if=/dev/zero of=/data/fill bs=1048576 count=80 2>&1",
        )
        .await;
        assert_ne!(fill.0, 0);
        assert!(
            fill.1.contains("No space left on device"),
            "exit={} output={}",
            fill.0,
            fill.1
        );
        assert_eq!(exec(&docker_test, &name, "rm /data/fill").await.0, 0);
        test_command.spec.revision = 2;
        test_command.operation_id = "quota-stop".into();
        test_command.action = Action::Stop;
        test_command.expected_runtime_id = first.runtime_id;
        let stopped = adapter
            .reconcile(
                &test_command,
                &LeaseGuard::new(test_command.clone(), Arc::new(Authority)),
            )
            .await
            .unwrap();
        assert_eq!(stopped.phase, Phase::Stopped);
        test_command.spec.revision = 3;
        test_command.operation_id = "quota-resume".into();
        test_command.action = Action::Apply;
        let resumed = adapter
            .reconcile(
                &test_command,
                &LeaseGuard::new(test_command.clone(), Arc::new(Authority)),
            )
            .await
            .unwrap();
        assert_eq!(resumed.phase, Phase::Running);
        assert_eq!(
            exec(&docker_test, &name, "cat /data/proof").await,
            (0, "retained-quota".into())
        );
        test_command.spec.revision = 4;
        test_command.operation_id = "quota-resize".into();
        test_command.expected_runtime_id = resumed.runtime_id;
        test_command.spec.volume.as_mut().unwrap().size_bytes = Some(134217728);
        assert!(
            adapter
                .reconcile(
                    &test_command,
                    &LeaseGuard::new(test_command.clone(), Arc::new(Authority))
                )
                .await
                .is_err()
        );
        assert_eq!(
            exec(&docker_test, &name, "cat /data/proof").await,
            (0, "retained-quota".into())
        );
        test_command.spec.volume = None;
        assert!(
            adapter
                .reconcile(
                    &test_command,
                    &LeaseGuard::new(test_command.clone(), Arc::new(Authority))
                )
                .await
                .is_err()
        );
        test_command.spec.volume = Some(RetainedVolume {
            mount_path: "/data".into(),
            size_bytes: Some(67108864),
        });
        let mut purge = test_command.clone();
        purge.action = Action::PurgeStorage;
        purge.operation_id = "premature-purge".into();
        purge.expected_runtime_id = None;
        assert!(
            adapter
                .reconcile(&purge, &LeaseGuard::new(purge.clone(), Arc::new(Authority)))
                .await
                .is_err()
        );
        test_command.operation_id = "quota-retire".into();
        test_command.action = Action::Retire;
        let retired = adapter
            .reconcile(
                &test_command,
                &LeaseGuard::new(test_command.clone(), Arc::new(Authority)),
            )
            .await
            .unwrap();
        assert_eq!(retired.phase, Phase::Absent);
        assert!(retired.ready);
        assert_eq!(
            retired.storage,
            Some(StorageObservation {
                volume_bytes: Some(67108864),
                retained: true
            })
        );
        docker_test
            .inspect_volume(&format!("{name}-data"))
            .await
            .unwrap();
        let replay = adapter
            .reconcile(
                &test_command,
                &LeaseGuard::new(test_command.clone(), Arc::new(Authority)),
            )
            .await
            .unwrap();
        assert_eq!(replay.storage, retired.storage);
        let inspected = adapter
            .inspect_completed(
                &test_command,
                &LeaseGuard::new(test_command.clone(), Arc::new(Authority)),
            )
            .await
            .unwrap();
        assert_eq!(inspected.storage, retired.storage);
        purge.spec.revision = 5;
        purge.operation_id = "quota-purge".into();
        let purged = adapter
            .reconcile(&purge, &LeaseGuard::new(purge.clone(), Arc::new(Authority)))
            .await
            .unwrap();
        assert_eq!(
            purged.storage,
            Some(StorageObservation {
                volume_bytes: Some(0),
                retained: false
            })
        );
        assert!(
            docker_test
                .inspect_volume(&format!("{name}-data"))
                .await
                .is_err()
        );
        assert_eq!(
            adapter
                .inspect_completed(&purge, &LeaseGuard::new(purge.clone(), Arc::new(Authority)))
                .await
                .unwrap()
                .storage,
            purged.storage
        );
        assert_eq!(
            adapter
                .reconcile(&purge, &LeaseGuard::new(purge.clone(), Arc::new(Authority)))
                .await
                .unwrap()
                .storage,
            purged.storage
        );
    })
    .await;
    let _ = docker
        .remove_container(
            &spec.name(),
            Some(
                RemoveContainerOptionsBuilder::default()
                    .force(true)
                    .v(false)
                    .build(),
            ),
        )
        .await;
    let _ = docker.remove_network(&format!("{}-net", spec.name())).await;
    let _ = docker
        .remove_volume(
            &format!("{}-data", spec.name()),
            None::<RemoveVolumeOptions>,
        )
        .await;
    command.lease_until = timestamp().unwrap() + 60;
    let client = StorageClient::new(helper_socket);
    let packet = StoragePacket::new("456", "123", "789", 1, 67108864, command.lease_until).unwrap();
    let purge = client.purge(&packet).await;
    let _ = stop_tx.send(());
    server.await.unwrap().unwrap();
    purge.unwrap();
    test.unwrap();
}
