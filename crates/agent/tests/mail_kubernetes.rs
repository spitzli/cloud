#![cfg(feature = "kubernetes")]
use webdock_agent::{
    mail::kubernetes::{KubernetesMailConfig, MailPhase, mail_objects},
    storage::StorageConfig,
};
use webdock_agent_protocol::mail::*;
fn claim() -> MailClaim {
    let material = MailMaterial::Credentials {
        bootstrap_password: Some("fixture-bootstrap".into()),
        username: "webdock-admin@mail-123.example.test".into(),
        password: "fixture-permanent".into(),
    };
    MailClaim {
        spec: MailSpec {
            version: 1,
            operation_id: "456".into(),
            customer_id: "123".into(),
            cluster_id: "789".into(),
            revision: 1,
            agent_generation: 1,
            instance_key: "mail-123".into(),
            hostname: "mail-123.example.test".into(),
            image: format!("registry.example.test/mail@sha256:{}", "a".repeat(64)),
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
fn config(claim: &MailClaim) -> KubernetesMailConfig {
    serde_json::from_value(serde_json::json!({"installation":"native-fixture","node":"fixture-node","image":claim.spec.image,"gatewayNamespace":"webdock-mail-gateway","retained":{}})).unwrap()
}
fn storage() -> StorageConfig {
    serde_json::from_value(serde_json::json!({"root":"/private/mail-fixture","node":"fixture-node","clusterID":"789","poolID":"123e4567-e89b-42d3-a456-426614174000","capacityBytes":21474836480_u64,"reserveBytes":67108864})).unwrap()
}
#[test]
fn mail_profile_retains_existing_names_and_restricted_storage() {
    let claim = claim();
    let config = config(&claim);
    let objects = mail_objects(&claim, &config, &storage(), MailPhase::Recovery).unwrap();
    assert!(
        objects
            .namespace
            .metadata
            .labels
            .as_ref()
            .unwrap()
            .values()
            .all(|value| value.len() <= 63),
        "Kubernetes label values are bounded"
    );
    assert_eq!(
        objects.namespace.metadata.name.as_deref(),
        Some("wd-mail-123")
    );
    assert_eq!(objects.deployment.metadata.name.as_deref(), Some("app-123"));
    assert_eq!(
        objects.claim.spec.as_ref().unwrap().volume_name.as_deref(),
        Some("wd-123-data")
    );
    assert_eq!(
        objects
            .volume
            .spec
            .as_ref()
            .unwrap()
            .persistent_volume_reclaim_policy
            .as_deref(),
        Some("Retain")
    );
    assert_eq!(
        objects
            .volume
            .spec
            .as_ref()
            .unwrap()
            .local
            .as_ref()
            .unwrap()
            .path,
        "/private/mail-fixture/123/mount/data"
    );
    let pod = objects
        .deployment
        .spec
        .as_ref()
        .unwrap()
        .template
        .spec
        .as_ref()
        .unwrap();
    assert_eq!(pod.automount_service_account_token, Some(false));
    assert_eq!(
        pod.security_context.as_ref().unwrap().run_as_user,
        Some(65532)
    );
    assert_eq!(
        pod.node_selector.as_ref().unwrap()["kubernetes.io/hostname"],
        "fixture-node"
    );
    assert_eq!(
        pod.containers[0].image_pull_policy.as_deref(),
        Some("IfNotPresent")
    );
    assert_eq!(
        pod.containers[0]
            .security_context
            .as_ref()
            .unwrap()
            .read_only_root_filesystem,
        Some(true)
    );
    assert!(
        pod.volumes
            .as_ref()
            .unwrap()
            .iter()
            .all(|volume| volume.host_path.is_none())
    );
    assert!(objects.secret.as_ref().unwrap().immutable.unwrap());
    assert!(
        !serde_json::to_string(&objects.deployment)
            .unwrap()
            .contains("fixture-bootstrap")
    );
    assert_eq!(
        objects.service.spec.as_ref().unwrap().type_.as_deref(),
        Some("ClusterIP")
    );
    assert_eq!(
        objects
            .service
            .spec
            .as_ref()
            .unwrap()
            .ports
            .as_ref()
            .unwrap()
            .iter()
            .map(|port| port.port)
            .collect::<Vec<_>>(),
        vec![8080, 2525, 2465, 1993]
    );
}
#[test]
fn mail_manifest_replay_does_not_change_pod_template_for_operation_or_revision() {
    let mut claim = claim();
    let config = config(&claim);
    let first = mail_objects(&claim, &config, &storage(), MailPhase::Normal).unwrap();
    claim.spec.operation_id = "457".into();
    claim.spec.revision = 2;
    claim.spec.action = MailAction::Suspend;
    let stopped = mail_objects(&claim, &config, &storage(), MailPhase::Normal).unwrap();
    assert_eq!(
        first.deployment.spec.as_ref().unwrap().template,
        stopped.deployment.spec.as_ref().unwrap().template
    );
    assert_eq!(stopped.deployment.spec.as_ref().unwrap().replicas, Some(0));
    assert!(stopped.secret.is_none());
    let denied = objects_policy(&stopped, "webdock-internal");
    assert_eq!(denied["spec"]["egress"], serde_json::json!([]));
    assert_eq!(denied["spec"]["ingress"], serde_json::json!([]));
}
fn objects_policy(
    objects: &webdock_agent::mail::kubernetes::MailObjects,
    name: &str,
) -> serde_json::Value {
    serde_json::to_value(
        objects
            .policies
            .iter()
            .find(|p| p.metadata.name.as_deref() == Some(name))
            .unwrap(),
    )
    .unwrap()
}
#[test]
fn mail_private_configuration_rejects_legacy_labels_without_explicit_uids() {
    let claim = claim();
    let mut config = config(&claim);
    config.retained=serde_json::from_value(serde_json::json!({"123":{"namespaceUID":"not-a-uid","deploymentUID":"not-a-uid","serviceUID":"not-a-uid","volumeUID":"not-a-uid","claimUID":"not-a-uid","policies":{}}})).unwrap();
    assert!(config.validate().is_err());
    assert!(serde_json::from_value::<KubernetesMailConfig>(serde_json::json!({"installation":"x","node":"n","image":claim.spec.image,"kubeconfig":"/arbitrary"})).is_err());
}

#[tokio::test]
async fn mail_live_denial_precedes_any_kubernetes_or_storage_access() {
    use std::os::unix::fs::PermissionsExt;
    use webdock_agent::mail::{MailAuthority, MailRuntime, kubernetes::KubernetesMailRuntime};
    use webdock_supervisor::{ExecutionError, Result};
    struct Denied;
    #[async_trait::async_trait]
    impl MailAuthority for Denied {
        async fn guard(&self, _: &MailClaim) -> Result<()> {
            Err(ExecutionError::Denied)
        }
    }
    let claim = claim();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mail.json");
    std::fs::write(&path, serde_json::to_vec(&config(&claim)).unwrap()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let client =
        kube::Client::try_from(kube::Config::new("https://127.0.0.1:1".parse().unwrap())).unwrap();
    let runtime = KubernetesMailRuntime::open(
        client,
        (
            webdock_agent::storage::StorageClient::new(directory.path().join("unreachable.sock")),
            storage(),
        ),
        path,
    )
    .unwrap();
    assert!(matches!(
        runtime.execute(&claim, &Denied).await,
        Err(ExecutionError::Denied)
    ));
}
