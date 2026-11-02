#![cfg(feature = "kubernetes")]
#[path = "support/configuration_fixture.rs"]
mod configuration_fixture;
#[path = "support/mail_adversarial.rs"]
mod mail_adversarial;
#[path = "support/mail_configuration.rs"]
mod mail_configuration;
#[path = "support/mail_data.rs"]
mod mail_data;
#[path = "support/mail_network.rs"]
mod mail_network;
#[path = "support/mail_retained.rs"]
mod mail_retained;
use k8s_openapi::api::{
    apps::v1::Deployment,
    core::v1::{Namespace, PersistentVolume, Pod},
};
use kube::{
    Api, Client, Config,
    api::{DeleteParams, ListParams, Patch, PatchParams, Preconditions},
    config::{KubeConfigOptions, Kubeconfig},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::Mutex;
use webdock_agent::{
    mail::{
        MailAuthority, MailRuntime,
        kubernetes::{KubernetesMailConfig, KubernetesMailRuntime},
    },
    storage::{StorageClient, StorageConfig, StoragePacket, StoragePool},
};
use webdock_agent_protocol::mail::*;
use webdock_supervisor::{ExecutionError, Result};
struct Authority {
    allowed: AtomicBool,
    accept: AtomicBool,
    checkpoints: Mutex<BTreeMap<String, MailCheckpoint>>,
}
#[async_trait::async_trait]
impl MailAuthority for Authority {
    async fn guard(&self, claim: &MailClaim) -> Result<()> {
        claim
            .validate(chrono::Utc::now())
            .map_err(|_| ExecutionError::Expired)?;
        if !self.allowed.load(Ordering::SeqCst) {
            return Err(ExecutionError::Denied);
        }
        Ok(())
    }
    async fn checkpoint(&self, claim: &MailClaim, checkpoint: MailCheckpoint) -> Result<()> {
        self.guard(claim).await?;
        let mut stored = self.checkpoints.lock().await;
        if stored
            .get(&claim.spec.operation_id)
            .is_some_and(|old| old.runtime_id != checkpoint.runtime_id)
        {
            return Err(ExecutionError::Denied);
        }
        stored.insert(claim.spec.operation_id.clone(), checkpoint);
        if !self.accept.load(Ordering::SeqCst) {
            return Err(ExecutionError::Journal);
        }
        Ok(())
    }
}
fn private_json(path: &Path, value: &impl serde::Serialize) {
    std::fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
fn fresh_claim(customer: String, image: String) -> MailClaim {
    let hostname = format!("mail-{customer}.fixture.test");
    let material = MailMaterial::Credentials {
        bootstrap_password: Some(uuid::Uuid::new_v4().to_string()),
        username: format!("webdock-admin@{hostname}"),
        password: uuid::Uuid::new_v4().to_string(),
    };
    MailClaim {
        spec: MailSpec {
            version: 1,
            operation_id: "7001".into(),
            customer_id: customer.clone(),
            cluster_id: "789".into(),
            revision: 1,
            agent_generation: 1,
            instance_key: format!("mail-{customer}"),
            hostname,
            image,
            resources: MailResources::FIXED,
            material_sha256: material.fingerprint().unwrap(),
            action: MailAction::Ensure,
        },
        material,
        lease: MailLease {
            lease_token: uuid::Uuid::new_v4().to_string(),
        },
        lease_until: chrono::Utc::now() + chrono::Duration::seconds(120),
        checkpoint: None,
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires explicitly owned Kind node and one quota-backed10GiB allocation"]
async fn owned_mail_kubernetes_lifecycle() {
    assert_eq!(
        std::env::var("WEBDOCK_MAIL_KIND_CASE").as_deref(),
        Ok("lifecycle")
    );
    run_fixture(false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires explicit owned Kind configuration fixture"]
async fn owned_mail_kubernetes_configuration() {
    assert_eq!(
        std::env::var("WEBDOCK_MAIL_KIND_CASE").as_deref(),
        Ok("configuration")
    );
    run_fixture(true).await;
}
async fn run_fixture(configuration_case: bool) {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("webdock_agent::mail::kubernetes=debug")
        .try_init();
    assert_eq!(
        std::env::var("WEBDOCK_MAIL_KIND_TEST").as_deref(),
        Ok("yes")
    );
    assert_eq!(rustix::process::geteuid().as_raw(), 0);
    let context = std::env::var("WEBDOCK_KUBE_TEST_CONTEXT").unwrap();
    assert_eq!(context, "kind-webdock-rust-native-3c8064");
    let config = Config::from_custom_kubeconfig(
        Kubeconfig::read_from(std::env::var("WEBDOCK_KUBE_TEST_CONFIG").unwrap()).unwrap(),
        &KubeConfigOptions {
            context: Some(context.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!config.accept_invalid_certs);
    let client = Client::try_from(config).unwrap();
    let base = PathBuf::from(std::env::var("WEBDOCK_MAIL_TEST_ROOT").unwrap());
    assert!(base.starts_with("/var/local/webdock-rust-tests"));
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let installation = format!("mailtest{}", &suffix[..12]);
    let customer = (u64::from_be_bytes(uuid::Uuid::new_v4().as_bytes()[..8].try_into().unwrap())
        % 1_000_000_000_000
        + 800_000_000_000_000)
        .to_string();
    let namespace = format!("wd-mail-{customer}");
    let volume = format!("wd-{customer}-data");
    let node = format!("{}-control-plane", context.trim_start_matches("kind-"));
    assert!(
        Api::<Namespace>::all(client.clone())
            .get_opt(&namespace)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        Api::<PersistentVolume>::all(client.clone())
            .get_opt(&volume)
            .await
            .unwrap()
            .is_none()
    );
    let root = base.join("volumes");
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let storage:StorageConfig=serde_json::from_value(json!({"root":root,"node":node,"clusterID":"789","poolID":uuid::Uuid::new_v4().to_string(),"capacityBytes":10737418240_u64,"reserveBytes":67108864})).unwrap();
    let storage_path = base.join("storage.json");
    private_json(&storage_path, &storage);
    StoragePool::initialize(&storage_path).unwrap();
    let socket = base.join("storage.sock");
    let server_socket = socket.clone();
    let (stop, shutdown) = tokio::sync::oneshot::channel();
    let helper = tokio::spawn(async move {
        webdock_agent::storage::serve(&storage_path, &server_socket, async {
            let _ = shutdown.await;
        })
        .await
    });
    for _ in 0..200 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let storage_client = StorageClient::new(socket);
    let image = std::env::var("WEBDOCK_MAIL_RUNTIME_IMAGE").unwrap();
    let gateway = format!("wd-mail-gateway-{}", &suffix[..8]);
    let peer = format!("wd-mail-peer-{}", &suffix[..8]);
    let configuration:KubernetesMailConfig=serde_json::from_value(json!({"installation":installation,"node":node,"image":image,"gatewayNamespace":gateway,"retained":{}})).unwrap();
    let path = base.join("runtime.json");
    private_json(&path, &configuration);
    let authority = Arc::new(Authority {
        allowed: AtomicBool::new(true),
        accept: AtomicBool::new(false),
        checkpoints: Mutex::new(BTreeMap::new()),
    });
    let mut claim = fresh_claim(customer.clone(), image);
    let work_client = client.clone();
    let work_storage = storage_client.clone();
    let work_config = storage.clone();
    let work_path = path.clone();
    let work_authority = authority.clone();
    let work_root = root.clone();
    let retained_cleanup = Arc::new(Mutex::new(BTreeMap::new()));
    let work_cleanup = retained_cleanup.clone();
    let work_gateway = gateway.clone();
    let work_peer = peer.clone();
    let work_installation = installation.clone();
    let test = tokio::spawn(async move {
        let network = if configuration_case {
            None
        } else {
            Some(
                mail_network::MailNetwork::create(
                    work_client.clone(),
                    work_gateway,
                    work_peer,
                    std::env::var("WEBDOCK_MAIL_PROBE_IMAGE").unwrap(),
                    work_installation.clone(),
                )
                .await,
            )
        };
        let runtime = KubernetesMailRuntime::open(
            work_client.clone(),
            (work_storage.clone(), work_config.clone()),
            work_path.clone(),
        )
        .unwrap();
        let mut capable = false;
        for _ in 0..60 {
            if runtime.capabilities().await.unwrap().is_some() {
                capable = true;
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        assert!(
            capable,
            "reviewed image cache, Ready node and storage pool must be proven"
        );
        claim.lease_until = chrono::Utc::now() + chrono::Duration::seconds(120);
        let first = runtime.execute(&claim, work_authority.as_ref()).await.err();
        assert!(
            matches!(first, Some(ExecutionError::Journal)),
            "initial ensure must reach custody checkpoint: {first:?}"
        );
        claim.checkpoint = work_authority
            .checkpoints
            .lock()
            .await
            .get(&claim.spec.operation_id)
            .cloned();
        assert!(claim.checkpoint.is_some());
        work_authority.accept.store(true, Ordering::SeqCst);
        let proof = runtime
            .execute(&claim, work_authority.as_ref())
            .await
            .unwrap();
        assert!(
            proof.running
                && proof.recovery_disabled
                && proof.submission_certificate_sha256.is_some()
        );
        for (kind, metadata) in [
            (
                "Namespace",
                Api::<Namespace>::all(work_client.clone())
                    .get(&proof.namespace)
                    .await
                    .unwrap()
                    .metadata,
            ),
            (
                "PersistentVolume",
                Api::<PersistentVolume>::all(work_client.clone())
                    .get(&format!("wd-{}-data", claim.spec.customer_id))
                    .await
                    .unwrap()
                    .metadata,
            ),
        ] {
            assert_eq!(
                metadata.labels.as_ref().unwrap()["webdock.dev/mail-installation"],
                work_installation
            );
            work_cleanup
                .lock()
                .await
                .insert(kind, metadata.uid.unwrap());
        }
        let draft = mail_data::RetainedDraft::create(&work_client, &claim).await;
        if configuration_case {
            let evidence = mail_configuration::verify(&work_client, &runtime, &claim, &proof).await;
            draft.verify(&work_client, &claim).await;
            claim.spec.operation_id = "7008".into();
            claim.spec.revision += 1;
            claim.spec.action = MailAction::Suspend;
            claim.lease.lease_token = uuid::Uuid::new_v4().to_string();
            claim.lease_until = chrono::Utc::now() + chrono::Duration::seconds(120);
            claim.checkpoint = None;
            runtime
                .execute(&claim, work_authority.as_ref())
                .await
                .unwrap();
            mail_configuration::deny_suspended(&work_client, &runtime, &claim, &proof).await;
            return evidence;
        }
        let network = network.expect("lifecycle case owns network probes");
        let deployments = Api::<Deployment>::namespaced(work_client.clone(), &proof.namespace);
        let name = format!("app-{}", claim.spec.customer_id);
        let before = deployments.get(&name).await.unwrap();
        let environment = before
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers[0]
            .env
            .as_ref()
            .unwrap();
        assert_eq!(
            environment.len(),
            1,
            "normal runtime must remove creation-owned recovery entries"
        );
        assert_eq!(environment[0].name, "STALWART_PUBLIC_URL");
        let pods = Api::<Pod>::namespaced(work_client.clone(), &proof.namespace);
        let before_pods = pods.list(&ListParams::default()).await.unwrap().items;
        assert_eq!(before_pods.len(), 1);
        let pod_uid = before_pods[0].metadata.uid.clone();
        let volume_state: Value =
            serde_json::from_slice(&std::fs::read(work_root.join("_pool/state.json")).unwrap())
                .unwrap();
        let repeated = runtime
            .execute(&claim, work_authority.as_ref())
            .await
            .unwrap();
        assert_eq!(proof.runtime_id, repeated.runtime_id);
        assert_eq!(
            proof.submission_certificate_sha256,
            repeated.submission_certificate_sha256
        );
        assert_eq!(
            pods.list(&ListParams::default()).await.unwrap().items[0]
                .metadata
                .uid,
            pod_uid
        );
        work_authority.allowed.store(false, Ordering::SeqCst);
        assert!(matches!(
            runtime.execute(&claim, work_authority.as_ref()).await,
            Err(ExecutionError::Denied)
        ));
        work_authority.allowed.store(true, Ordering::SeqCst);
        network
            .ingress(&proof.namespace, &claim.spec.customer_id)
            .await;
        deployments.patch(&name,&PatchParams::apply("webdock-mail-config"),&Patch::Apply(json!({"apiVersion":"apps/v1","kind":"Deployment","metadata":{"name":name,"namespace":proof.namespace},"spec":{"template":{"metadata":{"annotations":{"webdock.dev/submission-config":"fixture-retained"}}}}}))).await.unwrap();
        claim.spec.action = MailAction::Suspend;
        claim.spec.operation_id = "7002".into();
        claim.spec.revision = 2;
        claim.checkpoint = None;
        claim.lease.lease_token = uuid::Uuid::new_v4().to_string();
        claim.lease_until = chrono::Utc::now() + chrono::Duration::seconds(120);
        let suspended = runtime
            .execute(&claim, work_authority.as_ref())
            .await
            .unwrap();
        assert!(!suspended.running);
        assert!(
            pods.list(&ListParams::default())
                .await
                .unwrap()
                .items
                .is_empty()
        );
        let stopped = deployments.get(&name).await.unwrap();
        assert_eq!(stopped.metadata.uid, before.metadata.uid);
        assert_eq!(
            stopped
                .spec
                .as_ref()
                .unwrap()
                .template
                .metadata
                .as_ref()
                .unwrap()
                .annotations
                .as_ref()
                .unwrap()["webdock.dev/submission-config"],
            "fixture-retained"
        );
        let replicas_owned = stopped
            .metadata
            .managed_fields
            .as_ref()
            .unwrap()
            .iter()
            .any(|entry| {
                entry.manager.as_deref() == Some("webdock")
                    && entry
                        .fields_v1
                        .as_ref()
                        .is_some_and(|fields| fields.0["f:spec"].get("f:replicas").is_some())
            });
        assert!(replicas_owned, "SSA replica owner must remain webdock");
        mail_adversarial::denied_suspended_execution_override(
            work_client.clone(),
            &runtime,
            work_authority.as_ref(),
            &claim,
        )
        .await;
        network.suspended_egress(&proof.namespace).await;
        claim.spec.action = MailAction::Ensure;
        claim.spec.operation_id = "7003".into();
        claim.spec.revision = 3;
        claim.spec.agent_generation = 2;
        claim.lease.lease_token = uuid::Uuid::new_v4().to_string();
        claim.lease_until = chrono::Utc::now() + chrono::Duration::seconds(120);
        if let MailMaterial::Credentials {
            bootstrap_password, ..
        } = &mut claim.material
        {
            *bootstrap_password = None;
        }
        claim.spec.material_sha256 = claim.material.fingerprint().unwrap();
        let resumed = runtime
            .execute(&claim, work_authority.as_ref())
            .await
            .unwrap();
        assert_eq!(resumed.runtime_id, proof.runtime_id);
        draft.verify(&work_client, &claim).await;
        assert_eq!(
            resumed.submission_certificate_sha256,
            proof.submission_certificate_sha256
        );
        let runtime = mail_retained::adopt(
            work_client.clone(),
            (work_storage, work_config),
            &work_path,
            work_authority.as_ref(),
            &mut claim,
        )
        .await;
        draft.verify(&work_client, &claim).await;
        mail_adversarial::verify(
            work_client.clone(),
            &runtime,
            work_authority.as_ref(),
            &claim,
        )
        .await;
        let after: Value =
            serde_json::from_slice(&std::fs::read(work_root.join("_pool/state.json")).unwrap())
                .unwrap();
        assert_eq!(
            volume_state["volumes"][&claim.spec.customer_id]["uuid"],
            after["volumes"][&claim.spec.customer_id]["uuid"]
        );
        json!({"customerID":claim.spec.customer_id,"namespace":proof.namespace,"deploymentUID":proof.runtime_id,"initialPodUID":pod_uid,"stableReplay":true,"checkpointResponseLoss":true,"suspendResume":true,"reenrollment":true,"retainedJmapDraft":true,"networkIsolation":true,"adversarialOwnership":true,"executionOverrideDeniedWhileSuspended":true,"reviewedRetainedUidAdoption":true,"historicalNeverPodMigration":true,"retainedUdpServiceDenied":true,"submissionCertificateSha256":resumed.submission_certificate_sha256,"poolID":after["poolID"],"volumeUUID":after["volumes"][&claim.spec.customer_id]["uuid"]})
    });
    let outcome = test.await;
    mail_network::cleanup(client.clone(), &gateway, &installation).await;
    mail_network::cleanup(client.clone(), &peer, &installation).await;
    let namespaces = Api::<Namespace>::all(client.clone());
    if let Some(object) = namespaces.get_opt(&namespace).await.unwrap() {
        let known = retained_cleanup.lock().await;
        assert!(
            object
                .metadata
                .labels
                .as_ref()
                .and_then(|labels| labels.get("webdock.dev/mail-installation"))
                == Some(&installation)
                || known
                    .get("Namespace")
                    .is_some_and(|uid| object.metadata.uid.as_ref() == Some(uid))
        );
        drop(known);
        namespaces
            .delete(
                &namespace,
                &DeleteParams {
                    preconditions: Some(Preconditions {
                        uid: object.metadata.uid,
                        resource_version: object.metadata.resource_version,
                    }),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        for _ in 0..240 {
            if namespaces.get_opt(&namespace).await.unwrap().is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        assert!(namespaces.get_opt(&namespace).await.unwrap().is_none());
    }
    let volumes = Api::<PersistentVolume>::all(client.clone());
    if let Some(object) = volumes.get_opt(&volume).await.unwrap() {
        let known = retained_cleanup.lock().await;
        assert!(
            object
                .metadata
                .labels
                .as_ref()
                .and_then(|labels| labels.get("webdock.dev/mail-installation"))
                == Some(&installation)
                || known
                    .get("PersistentVolume")
                    .is_some_and(|uid| object.metadata.uid.as_ref() == Some(uid))
        );
        drop(known);
        volumes
            .delete(
                &volume,
                &DeleteParams {
                    preconditions: Some(Preconditions {
                        uid: object.metadata.uid,
                        resource_version: object.metadata.resource_version,
                    }),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        for _ in 0..120 {
            if volumes.get_opt(&volume).await.unwrap().is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        assert!(volumes.get_opt(&volume).await.unwrap().is_none());
    }
    let state: Value =
        serde_json::from_slice(&std::fs::read(root.join("_pool/state.json")).unwrap()).unwrap();
    if state["volumes"].get(&customer).is_some() {
        let packet = StoragePacket::new(
            &customer,
            &customer,
            "789",
            1,
            MailResources::FIXED.volume_bytes,
            u64::try_from((chrono::Utc::now() + chrono::Duration::seconds(120)).timestamp())
                .unwrap(),
        )
        .unwrap();
        storage_client.purge(&packet).await.unwrap();
    }
    stop.send(()).unwrap();
    helper.await.unwrap().unwrap();
    let evidence = match &outcome {
        Ok(evidence) => evidence.clone(),
        Err(_) => {
            json!({"customerID":customer,"namespace":namespace,"failed":true,"cleanupVerified":true})
        }
    };
    private_json(&base.join("evidence.json"), &evidence);
    std::fs::write(
        base.join("cleanup-complete"),
        b"owned namespace, PV and allocation removed",
    )
    .unwrap();
    outcome.unwrap();
}
