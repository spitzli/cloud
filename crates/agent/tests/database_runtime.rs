#![cfg(feature = "kubernetes")]
use webdock_agent::database::{DatabaseRuntimeConfig, backend_manifests};
use webdock_agent_protocol::database::*;
fn job() -> DatabaseJobSpec {
    serde_json::from_value(serde_json::json!({"version":1,"operationID":"101","agentGeneration":1,"operationGeneration":1,"clusterID":"789","action":"create","phase":"apply","revision":1,"previousPodUID":null,"backend":{"id":"456","kind":"dedicated","namespace":"wd-db-456","node":"fixture-node","poolID":"123e4567-e89b-42d3-a456-426614174000","revision":1,"cnpgUID":null,"resources":{"cpuMillicores":500,"memoryBytes":536870912,"volumeBytes":268435456},"host":"db-456.example.test","directPort":32000,"pooledPort":32001},"database":{"id":"123","kind":"dedicated","name":"db_123","revision":1,"credentialRevision":1,"connectionLimit":10},"members":[{"id":"123","projectID":"321","databaseName":"db_123","enabled":true}],"migration":null})).unwrap()
}
fn config() -> DatabaseRuntimeConfig {
    serde_json::from_value(serde_json::json!({"image":format!("ghcr.io/cloudnative-pg/postgresql:18.3@sha256:{}","a".repeat(64)),"poolerImage":format!("ghcr.io/cloudnative-pg/pgbouncer:1.25.1@sha256:{}","b".repeat(64)),"issuer":"test-database-ca","apiCIDRs":["10.96.0.1/32"],"podCIDRs":["192.168.0.0/16"],"storageSocket":"/run/webdock-postgres-storage/storage.sock","caFile":"/private/database-ca.crt"})).unwrap()
}
#[test]
fn native_database_objects_preserve_cnpg_names_resources_roles_and_storage() {
    let job = job();
    let objects = backend_manifests(&job, &config()).unwrap();
    let cluster = objects.iter().find(|o| o["kind"] == "Cluster").unwrap();
    assert_eq!(cluster["metadata"]["name"], "postgres");
    assert_eq!(cluster["metadata"]["namespace"], "wd-db-456");
    assert_eq!(cluster["spec"]["postgresUID"], 26);
    assert_eq!(
        cluster["spec"]["storage"]["pvcTemplate"]["volumeName"],
        "wd-db-456"
    );
    assert_eq!(
        cluster["spec"]["postgresql"]["pg_hba"],
        serde_json::json!([
            "hostssl db_123 wd_migrate_123,wd_app_123,wd_export_123 all scram-sha-256",
            "host all all all reject"
        ])
    );
    assert_eq!(
        cluster["spec"]["resources"]["limits"]["memory"],
        "536870912"
    );
    assert!(!cluster["spec"]["enableSuperuserAccess"].as_bool().unwrap());
    let policy = objects
        .iter()
        .find(|o| o["metadata"]["name"] == "default-deny")
        .unwrap();
    assert_eq!(
        policy["spec"]["policyTypes"],
        serde_json::json!(["Ingress", "Egress"])
    );
    let certificate = objects.iter().find(|o| o["kind"] == "Certificate").unwrap();
    assert_eq!(certificate["spec"]["issuerRef"]["name"], "test-database-ca");
    let mut foreign = job.clone();
    foreign.backend.namespace = "foreign".into();
    assert!(backend_manifests(&foreign, &config()).is_err());
    let mut invalid = config();
    invalid.api_cidrs = vec!["10.96.0.1/24".into()];
    assert!(invalid.validate().is_err());
    invalid = config();
    invalid.image = invalid.image.replace(":18.3", ":17.9");
    assert!(invalid.validate().is_err());
}
#[test]
fn platform_database_maintenance_and_storage_routes_are_operator_bound() {
    let mut config = config();
    config
        .platform_databases
        .insert("123".into(), vec!["webdock_runtime".into()]);
    let mut job = job();
    let objects = backend_manifests(&job, &config).unwrap();
    let cluster = objects.iter().find(|o| o["kind"] == "Cluster").unwrap();
    assert_eq!(
        cluster["spec"]["postgresql"]["pg_hba"][0],
        "hostssl db_123 wd_migrate_123,wd_app_123,wd_export_123,webdock_runtime all scram-sha-256"
    );
    job.action = DatabaseAction::RotateCredentials;
    assert!(config.admit(&job).is_err());
    job.action = DatabaseAction::Create;
    job.backend.kind = "shared-pool".into();
    job.database.as_mut().unwrap().kind = "shared".into();
    assert!(backend_manifests(&job, &config).is_err());
    config.storage_socket = "/var/run/docker.sock".into();
    assert!(config.validate().is_err());
}

#[tokio::test]
async fn database_execution_denies_before_touching_an_unreachable_cluster() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let ca = directory.path().join("ca.crt");
    let path = directory.path().join("config.json");
    let cert = rcgen::generate_simple_self_signed(vec!["database.fixture.test".into()])
        .unwrap()
        .cert
        .pem();
    std::fs::write(&ca, cert).unwrap();
    std::fs::set_permissions(&ca, std::fs::Permissions::from_mode(0o644)).unwrap();
    let mut configuration = config();
    configuration.ca_file = ca;
    std::fs::write(&path, serde_json::to_vec(&configuration).unwrap()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let client =
        kube::Client::try_from(kube::Config::new("https://127.0.0.1:1".parse().unwrap())).unwrap();
    let executor = webdock_agent::database::NativeDatabaseExecutor::open(client, path).unwrap();
    let claim = DatabaseClaim {
        spec: job(),
        lease: DatabaseLease {
            operation_id: "101".into(),
            generation: 1,
            lease_token: "a".repeat(64),
        },
        lease_until: chrono::Utc::now() + chrono::Duration::seconds(60),
        material: DatabaseMaterial {
            credentials: Some(DatabaseCredentials {
                application_password: "a".repeat(43),
                migration_password: "b".repeat(43),
            }),
            transfer_password: None,
            object_store: None,
        },
    };
    assert!(matches!(
        executor
            .execute(&claim, &webdock_agent::database::DenyDatabaseAuthority)
            .await,
        Err(webdock_supervisor::ExecutionError::Denied)
    ));
}

#[tokio::test]
#[ignore = "requires explicitly owned Kind node, root helper, approved CNPG images and private fixture CA"]
async fn disposable_database_apply_preserves_physical_uid_and_checkpoint_fence() {
    use k8s_openapi::api::core::v1::{Namespace, PersistentVolume};
    use kube::{
        Api, Client, Config,
        api::{ApiResource, DeleteParams, DynamicObject, GroupVersionKind},
        config::{KubeConfigOptions, Kubeconfig},
    };
    use std::{
        os::unix::fs::PermissionsExt,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };
    use webdock_agent::{
        database::{DatabaseAuthority, DatabaseStep, NativeDatabaseExecutor},
        storage::{
            StorageKind, StoragePool,
            database::{DatabaseStorageClient, DatabaseStoragePacket},
        },
    };
    assert_eq!(
        std::env::var("WEBDOCK_DISPOSABLE_DATABASE_TEST").as_deref(),
        Ok("yes")
    );
    assert_eq!(rustix::process::geteuid().as_raw(), 0);
    let context = std::env::var("WEBDOCK_KUBE_TEST_CONTEXT").unwrap();
    assert!(context.starts_with("kind-webdock-rust-native-"));
    let client = Client::try_from(
        Config::from_custom_kubeconfig(
            Kubeconfig::read_from(std::env::var("WEBDOCK_KUBE_TEST_CONFIG").unwrap()).unwrap(),
            &KubeConfigOptions {
                context: Some(context.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    )
    .unwrap();
    let numeric = u64::from_be_bytes(uuid::Uuid::new_v4().as_bytes()[..8].try_into().unwrap())
        % 1_000_000_000_000
        + 1000;
    let base = tempfile::Builder::new()
        .prefix("webdock-db-runtime-owned-")
        .tempdir()
        .unwrap()
        .keep();
    std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = base.join("volumes");
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let node = context.trim_start_matches("kind-").to_owned() + "-control-plane";
    let pool = uuid::Uuid::new_v4().to_string();
    let storage_config = base.join("storage.json");
    std::fs::write(&storage_config,serde_json::to_vec(&serde_json::json!({"root":root,"node":node,"clusterID":"789","poolID":pool,"capacityBytes":2147483648_u64,"reserveBytes":67108864})).unwrap()).unwrap();
    std::fs::set_permissions(&storage_config, std::fs::Permissions::from_mode(0o600)).unwrap();
    StoragePool::initialize(&storage_config).unwrap();
    let socket_root = std::path::Path::new("/run/webdock-postgres-storage");
    if !socket_root.exists() {
        std::fs::create_dir(socket_root).unwrap();
        std::fs::set_permissions(socket_root, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let socket = socket_root.join(format!("owned-{numeric}.sock"));
    let server_socket = socket.clone();
    let (stop, shutdown) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        webdock_agent::storage::serve_kind(
            &storage_config,
            &server_socket,
            StorageKind::Database,
            async {
                let _ = shutdown.await;
            },
        )
        .await
    });
    for _ in 0..200 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let mut configuration = config();
    configuration.ca_file = std::env::var("WEBDOCK_DATABASE_TEST_CA").unwrap().into();
    configuration.issuer = "webdock-runtime-database-ca".into();
    configuration.storage_socket = socket.clone();
    configuration.image = std::env::var("WEBDOCK_DATABASE_TEST_IMAGE").unwrap();
    configuration.pooler_image = std::env::var("WEBDOCK_DATABASE_TEST_POOLER_IMAGE").unwrap();
    configuration.api_cidrs =
        serde_json::from_str(&std::env::var("WEBDOCK_DATABASE_TEST_API_CIDRS").unwrap()).unwrap();
    let path = base.join("runtime.json");
    std::fs::write(&path, serde_json::to_vec(&configuration).unwrap()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut spec = job();
    spec.backend.id = numeric.to_string();
    spec.backend.namespace = format!("wd-db-{numeric}");
    spec.backend.host = format!("db-{numeric}.fixture.test");
    spec.backend.node = node;
    spec.backend.pool_id = pool.clone();
    spec.backend.resources.volume_bytes = 536870912;
    use std::io::Write;
    let hosts_marker = format!("webdock-owned-database-{numeric}");
    let host_line = format!(
        "\n{} {} db-{numeric}.wrong.fixture.test # {hosts_marker}\n",
        std::env::var("WEBDOCK_DATABASE_TEST_NODE_IP").unwrap(),
        spec.backend.host
    );
    std::fs::OpenOptions::new()
        .append(true)
        .open("/etc/hosts")
        .unwrap()
        .write_all(host_line.as_bytes())
        .unwrap();
    let namespace = spec.backend.namespace.clone();
    let volume_name = format!("wd-db-{numeric}");
    let packet = DatabaseStoragePacket {
        version: 1,
        installation_id: webdock_core::id::RecordId::parse(&spec.backend.id).unwrap(),
        cluster_id: webdock_core::id::RecordId::parse("789").unwrap(),
        revision: 1,
        node: spec.backend.node.clone(),
        pool_id: pool,
        volume_bytes: spec.backend.resources.volume_bytes,
        lease_until: chrono::Utc::now() + chrono::Duration::seconds(300),
    };
    struct OwnedFixtureAuthority {
        backend: String,
        allowed: AtomicBool,
        checkpoints: tokio::sync::Mutex<Vec<DatabaseCheckpoint>>,
        client: kube::Client,
        resume_guards: std::sync::atomic::AtomicUsize,
    }
    #[async_trait::async_trait]
    impl DatabaseAuthority for OwnedFixtureAuthority {
        async fn guard(&self, c: &DatabaseClaim) -> webdock_supervisor::Result<()> {
            if self.allowed.load(Ordering::SeqCst)
                && c.spec
                    .operation_id
                    .parse::<u64>()
                    .is_ok_and(|id| (101..=108).contains(&id))
                && c.spec.backend.id == self.backend
            {
                if c.spec.action == DatabaseAction::Resume
                    && c.spec.phase == DatabaseExecutionPhase::Apply
                    && self.resume_guards.fetch_add(1, Ordering::SeqCst) == 1
                {
                    let resource = ApiResource::from_gvk(&GroupVersionKind::gvk(
                        "postgresql.cnpg.io",
                        "v1",
                        "Cluster",
                    ));
                    let api: Api<DynamicObject> = Api::namespaced_with(
                        self.client.clone(),
                        &c.spec.backend.namespace,
                        &resource,
                    );
                    let object = api.get("postgres").await.unwrap();
                    assert_eq!(object.metadata.uid, c.spec.backend.cnpg_uid);
                    api.patch("postgres",&kube::api::PatchParams::default(),&kube::api::Patch::Merge(serde_json::json!({"metadata":{"uid":object.metadata.uid,"resourceVersion":object.metadata.resource_version,"annotations":{"webdock.dev/fixture-observer":"changed-during-storage-check"}}}))).await.unwrap();
                }
                Ok(())
            } else {
                Err(webdock_supervisor::ExecutionError::Denied)
            }
        }
        async fn checkpoint(
            &self,
            c: &DatabaseClaim,
            p: DatabaseCheckpoint,
        ) -> webdock_supervisor::Result<()> {
            self.guard(c).await?;
            self.checkpoints.lock().await.push(p);
            Ok(())
        }
    }
    let authority = Arc::new(OwnedFixtureAuthority {
        backend: spec.backend.id.clone(),
        allowed: AtomicBool::new(true),
        checkpoints: tokio::sync::Mutex::new(vec![]),
        client: client.clone(),
        resume_guards: std::sync::atomic::AtomicUsize::new(0),
    });
    let executor = NativeDatabaseExecutor::open(client.clone(), path).unwrap();
    let capacity = executor
        .capabilities(std::slice::from_ref(&packet.node))
        .await
        .unwrap()
        .expect("healthy admitted operator and verified storage must advertise database capacity");
    assert_eq!(capacity.database_version, 1);
    assert_eq!(capacity.database_pools.len(), 1);
    assert_eq!(capacity.database_pools[0].storage.pool_id, packet.pool_id);

    let task_client = client.clone();
    let task_root = root.clone();
    let test=tokio::spawn(async move {
        let mut claim=DatabaseClaim{spec,lease:DatabaseLease{operation_id:"101".into(),generation:1,lease_token:"a".repeat(64)},lease_until:chrono::Utc::now()+chrono::Duration::seconds(300),material:DatabaseMaterial{credentials:Some(DatabaseCredentials{application_password:"a".repeat(43),migration_password:"b".repeat(43)}),transfer_password:None,object_store: None,}};
        assert!(matches!(executor.execute(&claim,authority.as_ref()).await.unwrap(),DatabaseStep::Pending));
        let uid=authority.checkpoints.lock().await[0].cnpg_uid.clone();assert_eq!(authority.checkpoints.lock().await[0].phase,DatabaseCheckpointPhase::Applied);
        let resource=ApiResource::from_gvk(&GroupVersionKind::gvk("postgresql.cnpg.io","v1","Cluster"));let clusters:Api<DynamicObject>=Api::namespaced_with(task_client,&claim.spec.backend.namespace,&resource);
        let mut ready=false;for _ in 0..180 {let c=clusters.get("postgres").await.unwrap();assert_eq!(c.metadata.uid.as_deref(),Some(uid.as_str()));if c.data["status"]["readyInstances"]==1 {ready=true;break}tokio::time::sleep(std::time::Duration::from_secs(1)).await;}assert!(ready,"owned CNPG instance must actually become ready");
        let before=std::fs::read(task_root.join("_pool/state.json")).unwrap();claim.lease_until=chrono::Utc::now()+chrono::Duration::seconds(300);assert!(matches!(executor.execute(&claim,authority.as_ref()).await.unwrap(),DatabaseStep::Pending));assert_eq!(authority.checkpoints.lock().await[1].cnpg_uid,uid);assert_eq!(before,std::fs::read(task_root.join("_pool/state.json")).unwrap());
        claim.spec.backend.cnpg_uid=Some(uid.clone());claim.spec.phase=DatabaseExecutionPhase::Configure;
        assert!(matches!(executor.execute(&claim,authority.as_ref()).await.unwrap(),DatabaseStep::Pending));
        let checkpoints=authority.checkpoints.lock().await;assert_eq!(checkpoints[2].phase,DatabaseCheckpointPhase::Configuring);assert_eq!(checkpoints[3].phase,DatabaseCheckpointPhase::Observing);drop(checkpoints);
        claim.spec.phase=DatabaseExecutionPhase::Observe;let mut completed=false;
        for _ in 0..90 {match executor.execute(&claim,authority.as_ref()).await.unwrap(){DatabaseStep::Complete(proof)=>{assert_eq!(proof.state,DatabaseProofState::Ready);assert!(proof.endpoint_ready);assert_eq!(proof.cnpg_uid,uid);completed=true;break;},DatabaseStep::Pending=>tokio::time::sleep(std::time::Duration::from_millis(500)).await}}
        assert!(completed,"both database roles must authenticate over verified public direct/pooled TLS endpoints");
        claim.material.credentials.as_mut().unwrap().application_password="c".repeat(43);assert!(matches!(executor.execute(&claim,authority.as_ref()).await.unwrap(),DatabaseStep::Pending));claim.material.credentials.as_mut().unwrap().application_password="a".repeat(43);
        let hostname=claim.spec.backend.host.clone();claim.spec.backend.host=format!("db-{numeric}.wrong.fixture.test");assert!(matches!(executor.execute(&claim,authority.as_ref()).await.unwrap(),DatabaseStep::Pending));claim.spec.backend.host=hostname;

        for action in [DatabaseAction::Pause, DatabaseAction::Resume, DatabaseAction::Restart, DatabaseAction::RotateCredentials, DatabaseAction::Resize, DatabaseAction::Delete, DatabaseAction::Purge] {
            claim.spec.action=action;
            claim.spec.operation_id=(claim.spec.operation_id.parse::<u64>().unwrap()+1).to_string();
            claim.lease.operation_id=claim.spec.operation_id.clone();
            claim.spec.previous_pod_uid=None;
            if action==DatabaseAction::RotateCredentials {
                claim.spec.database.as_mut().unwrap().credential_revision+=1;
                claim.material.credentials.as_mut().unwrap().application_password="d".repeat(43);
                claim.material.credentials.as_mut().unwrap().migration_password="e".repeat(43);
            }
            if action==DatabaseAction::Resize {
                claim.spec.backend.resources.cpu_millicores=750;
                claim.spec.backend.resources.memory_bytes=805306368;
            }
            claim.lease_until=chrono::Utc::now()+chrono::Duration::seconds(300);
            claim.spec.phase=DatabaseExecutionPhase::Apply;
            assert!(matches!(executor.execute(&claim,authority.as_ref()).await.unwrap_or_else(|error|panic!("{action:?} apply: {error:?}")),DatabaseStep::Pending));
            if action==DatabaseAction::Restart {
                claim.spec.previous_pod_uid=authority.checkpoints.lock().await.last().unwrap().previous_pod_uid.clone();
                assert!(claim.spec.previous_pod_uid.is_some());
            }
            claim.spec.phase=DatabaseExecutionPhase::Configure;
            let mut configured=false;
            for _ in 0..180 {
                executor.execute(&claim,authority.as_ref()).await.unwrap_or_else(|error|panic!("{action:?} configure: {error:?}"));
                if authority.checkpoints.lock().await.last().is_some_and(|p|p.phase==DatabaseCheckpointPhase::Observing) { configured=true;break; }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            assert!(configured,"maintenance action must reach observation: {action:?}");
            claim.spec.phase=DatabaseExecutionPhase::Observe;
            let expected=match action {DatabaseAction::Pause=>DatabaseProofState::Paused,DatabaseAction::Resume|DatabaseAction::Restart|DatabaseAction::Resize|DatabaseAction::RotateCredentials=>DatabaseProofState::Ready,DatabaseAction::Delete=>DatabaseProofState::Retained,DatabaseAction::Purge=>DatabaseProofState::Purged,_=>unreachable!()};
            let mut complete=false;
            for _ in 0..180 {
                match executor.execute(&claim,authority.as_ref()).await.unwrap() {DatabaseStep::Complete(proof)=>{assert_eq!(proof.state,expected);complete=true;break;},DatabaseStep::Pending=>tokio::time::sleep(std::time::Duration::from_millis(500)).await}
            }
            assert!(complete,"maintenance action must complete: {action:?}");
            if action!=DatabaseAction::Purge { assert!(task_root.join(format!("{}/volume.ext4",claim.spec.backend.id)).exists()); }
        }
        assert!(!task_root.join(format!("{}/volume.ext4",claim.spec.backend.id)).exists());
        claim.spec.action=DatabaseAction::Create;
        claim.spec.phase=DatabaseExecutionPhase::Apply;
        claim.spec.backend.cnpg_uid=Some("foreign-runtime-uid".into());assert!(matches!(executor.execute(&claim,authority.as_ref()).await,Err(webdock_supervisor::ExecutionError::Ownership)));
        authority.allowed.store(false,Ordering::SeqCst);assert!(matches!(executor.execute(&claim,authority.as_ref()).await,Err(webdock_supervisor::ExecutionError::Denied)));
        eprintln!("owned CNPG Apply/Configure/Observe: SCRAM roles and verified TLS direct/pooled logins; retained UID/filesystem; restart/credential rotation/resize/pause/resume/delete/purge; changed UID/revoked guard denied");
    }).await;
    let hosts = std::fs::read_to_string("/etc/hosts")
        .unwrap()
        .lines()
        .filter(|line| !line.ends_with(&format!("# {hosts_marker}")))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write("/etc/hosts", hosts).unwrap();
    let namespaces: Api<Namespace> = Api::all(client.clone());
    let _ = namespaces
        .delete(&namespace, &DeleteParams::default())
        .await;
    let mut gone = false;
    for _ in 0..240 {
        if namespaces.get_opt(&namespace).await.unwrap().is_none() {
            gone = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    if !gone {
        let _ = stop.send(());
        let _ = server.await;
        panic!(
            "preserving owned database pool {} until namespace termination",
            base.display()
        );
    }
    let volumes: Api<PersistentVolume> = Api::all(client);
    if let Some(volume) = volumes.get_opt(&volume_name).await.unwrap() {
        assert_eq!(
            volume.metadata.labels.as_ref().unwrap()["webdock.dev/database-installation"],
            numeric.to_string()
        );
        volumes
            .delete(
                &volume_name,
                &DeleteParams {
                    preconditions: Some(kube::api::Preconditions {
                        uid: volume.metadata.uid,
                        resource_version: volume.metadata.resource_version,
                    }),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }
    let mut packet = packet;
    packet.lease_until = chrono::Utc::now() + chrono::Duration::seconds(120);
    let purge = DatabaseStorageClient::new(socket).purge(&packet).await;
    let _ = stop.send(());
    server.await.unwrap().unwrap();
    purge.unwrap();
    std::fs::remove_dir_all(&base).unwrap();
    test.unwrap();
}

#[test]
fn database_dispatch_rejects_tampering_expiry_and_enrollment_mismatch() {
    use webdock_agent::management::database_claim;
    let spec = job();
    let wire = webdock_agent_protocol::DatabaseCommand {
        request_sha256: spec.fingerprint().unwrap(),
        spec_json: serde_json::to_vec(&spec).unwrap(),
        lease_json: serde_json::to_vec(&DatabaseLease {
            operation_id: "101".into(),
            generation: 1,
            lease_token: "a".repeat(64),
        })
        .unwrap(),
        lease_until: (chrono::Utc::now() + chrono::Duration::seconds(60)).timestamp() as u64,
        material_json: serde_json::to_vec(&DatabaseMaterial {
            credentials: Some(DatabaseCredentials {
                application_password: "a".repeat(43),
                migration_password: "b".repeat(43),
            }),
            transfer_password: None,
            object_store: None,
        })
        .unwrap(),
    };
    assert!(database_claim(&wire, 1).is_ok());
    assert!(database_claim(&wire, 2).is_err());
    let mut bad = wire.clone();
    bad.request_sha256 = "b".repeat(64);
    assert!(database_claim(&bad, 1).is_err());
    let mut bad = wire.clone();
    bad.lease_until = 1;
    assert!(database_claim(&bad, 1).is_err());
    let mut bad = wire.clone();
    bad.material_json = vec![b' '; 65537];
    assert!(database_claim(&bad, 1).is_err());
}

#[test]
fn backup_manifest_requires_operator_pins_and_keeps_retention_off_on_revoke() {
    let mut value = serde_json::to_value(job()).unwrap();
    value["action"] = serde_json::json!("configureBackup");
    value["backend"]["cnpgUID"] = serde_json::json!("fixture-source-uid");
    value["backup"] = serde_json::json!({"id":"1001","installationID":"456","customerID":"1002","projectID":"321","sourceCnpgUID":"fixture-source-uid","revision":2,"resourceRevision":1,"destination":{"id":"1003","endpointURL":"https://s3.example.invalid","region":"eu-central-1","bucket":"fixture-backup","archivePath":"postgresql/1002/321/456/1001","serverName":"wd-456-1001","credentialID":"1004","credentialRevision":1,"recoveryCredentialID":"1005","recoveryCredentialRevision":1,"keyRecoveryID":"1006"},"schedule":"0 0 1 * * *","suspended":true,"walEnabled":true,"retentionDays":30,"retentionDelete":false,"sidecar":{"cpuMillicores":100,"memoryBytes":134217728,"ephemeralBytes":268435456},"recoveryJob":{"cpuMillicores":500,"memoryBytes":536870912,"ephemeralBytes":536870912}});
    let job: DatabaseJobSpec = serde_json::from_value(value).unwrap();
    let material = DatabaseObjectStoreMaterial {
        access_key_id: "fixture-write".into(),
        secret_access_key: "private-fixture-key-only".into(),
        read_only: false,
    };
    assert!(webdock_agent::database::backup_manifests(&job, &config(), &material).is_err());
    let mut c = serde_json::to_value(config()).unwrap();
    c["backup"] = serde_json::json!({"pluginImage":format!("localhost/fixture-plugin@sha256:{}","a".repeat(64)),"sidecarImage":format!("localhost/fixture-sidecar@sha256:{}","b".repeat(64)),"endpoints":["https://s3.example.invalid"],"egressCIDRs":["192.0.2.10/32"]});
    let endpoint_ca = rcgen::generate_simple_self_signed(vec!["s3.example.invalid".into()])
        .unwrap()
        .cert
        .pem();
    c["backup"]["endpointCACertificate"] = serde_json::json!(endpoint_ca);
    let c: DatabaseRuntimeConfig = serde_json::from_value(c).unwrap();
    let mut private_trust = c.clone();
    private_trust
        .backup
        .as_mut()
        .unwrap()
        .endpoint_ca_certificate = Some(format!(
        "{}{}",
        endpoint_ca,
        rcgen::KeyPair::generate().unwrap().serialize_pem()
    ));
    assert!(
        private_trust.validate().is_err(),
        "public endpoint CA configuration must reject private key material"
    );
    let objects = webdock_agent::database::backup_manifests(&job, &c, &material).unwrap();
    let store = objects.iter().find(|o| o["kind"] == "ObjectStore").unwrap();
    assert!(store["spec"]["configuration"].get("serverName").is_none());
    assert!(store["spec"]["retentionPolicy"].is_null());
    assert_eq!(
        store["spec"]["configuration"]["endpointCA"],
        serde_json::json!({"name":"s3-ca-1003","key":"ca.crt"})
    );
    let ca = objects
        .iter()
        .find(|o| o["metadata"]["name"] == "s3-ca-1003")
        .unwrap();
    use base64::Engine;
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(ca["data"]["ca.crt"].as_str().unwrap())
            .unwrap(),
        endpoint_ca.as_bytes()
    );
    let schedule = objects
        .iter()
        .find(|o| o["kind"] == "ScheduledBackup")
        .unwrap();
    assert_eq!(schedule["spec"]["suspend"], true);
    let cluster = objects.iter().find(|o| o["kind"] == "Cluster").unwrap();
    assert_eq!(
        cluster["spec"]["plugins"][0]["parameters"]["serverName"],
        "wd-456-1001"
    );
    assert_eq!(cluster["spec"]["plugins"][0]["isWALArchiver"], true);
    let quota = objects
        .iter()
        .find(|o| o["kind"] == "ResourceQuota")
        .unwrap();
    assert_eq!(quota["spec"]["hard"]["requests.cpu"], "1300m");
}
#[test]
fn recovery_manifest_uses_read_only_source_and_exact_base_time_and_timeline() {
    let mut value = serde_json::to_value(job()).unwrap();
    let policy = serde_json::json!({"id":"1001","installationID":"999","customerID":"1002","projectID":"321","sourceCnpgUID":"fixture-source-uid","revision":2,"resourceRevision":1,"destination":{"id":"1003","endpointURL":"https://s3.example.invalid","region":"eu-central-1","bucket":"fixture-backup","archivePath":"postgresql/1002/321/999/1001","serverName":"wd-999-1001","credentialID":"1004","credentialRevision":1,"recoveryCredentialID":"1005","recoveryCredentialRevision":1,"keyRecoveryID":"1006"},"schedule":"0 0 1 * * *","suspended":true,"walEnabled":true,"retentionDays":30,"retentionDelete":true,"sidecar":{"cpuMillicores":100,"memoryBytes":134217728,"ephemeralBytes":268435456},"recoveryJob":{"cpuMillicores":500,"memoryBytes":536870912,"ephemeralBytes":536870912}});
    value["action"] = serde_json::json!("restore");
    value["recovery"] = serde_json::json!({"source":policy,"sourceDatabaseID":"1007","baseBackupId":"20261025T010203","targetTime":"2026-10-25T02:00:00Z","timeline":1,"credentialRevision":1,"verificationSHA256":"a".repeat(64)});
    let job: DatabaseJobSpec = serde_json::from_value(value).unwrap();
    let mut c = serde_json::to_value(config()).unwrap();
    c["backup"] = serde_json::json!({"pluginImage":format!("localhost/fixture-plugin@sha256:{}","a".repeat(64)),"sidecarImage":format!("localhost/fixture-sidecar@sha256:{}","b".repeat(64)),"endpoints":["https://s3.example.invalid"],"egressCIDRs":["192.0.2.10/32"]});
    let c: DatabaseRuntimeConfig = serde_json::from_value(c).unwrap();
    let objects = backend_manifests(&job, &c).unwrap();
    let cluster = objects.iter().find(|o| o["kind"] == "Cluster").unwrap();
    assert_eq!(
        cluster["spec"]["bootstrap"]["recovery"]["recoveryTarget"],
        serde_json::json!({"backupID":"20261025T010203","targetTime":"2026-10-25T02:00:00+00:00","targetTLI":"1"})
    );
    assert!(cluster["spec"].get("plugins").is_none());
    assert_eq!(
        cluster["spec"]["externalClusters"][0]["plugin"]["parameters"]["serverName"],
        "wd-999-1001"
    );
    let material = DatabaseObjectStoreMaterial {
        access_key_id: "fixture-read".into(),
        secret_access_key: "fixture-read-secret-only".into(),
        read_only: true,
    };
    let objects = webdock_agent::database::backup_manifests(&job, &c, &material).unwrap();
    assert!(!objects.iter().any(|o| o["kind"] == "ScheduledBackup"));
    assert!(
        objects.iter().find(|o| o["kind"] == "ObjectStore").unwrap()["spec"]["retentionPolicy"]
            .is_null()
    );
}
