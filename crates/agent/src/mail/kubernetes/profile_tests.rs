use super::lifecycle::validate_workload;
use super::*;
use kube::api::DynamicObject;
use serde_json::{Value, json};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use webdock_agent_protocol::mail::*;
fn fixture() -> (
    MailClaim,
    KubernetesMailConfig,
    StorageConfig,
    DynamicObject,
) {
    let material = MailMaterial::Credentials {
        bootstrap_password: None,
        username: "webdock-admin@mail-123.example.test".into(),
        password: "fixture-only".into(),
    };
    let claim = MailClaim {
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
    };
    let mut config: KubernetesMailConfig = serde_json::from_value(json!({"installation":"fixture","node":"fixture-node","image":claim.spec.image,"retained":{}})).unwrap();
    let storage = serde_json::from_value(json!({"root":"/private/fixture","node":"fixture-node","clusterID":"789","poolID":uuid::Uuid::new_v4().to_string(),"capacityBytes":10737418240_u64,"reserveBytes":67108864})).unwrap();
    config.retained.insert(
        "123".into(),
        RetainedBinding {
            namespace_uid: uuid::Uuid::new_v4().to_string(),
            deployment_uid: uuid::Uuid::new_v4().to_string(),
            service_uid: uuid::Uuid::new_v4().to_string(),
            volume_uid: uuid::Uuid::new_v4().to_string(),
            claim_uid: uuid::Uuid::new_v4().to_string(),
            bootstrap_secret: None,
            policies: Default::default(),
        },
    );
    let objects = mail_objects(&claim, &config, &storage, MailPhase::Normal).unwrap();
    let mut deployment: DynamicObject =
        serde_json::from_value(serde_json::to_value(objects.deployment).unwrap()).unwrap();
    deployment.metadata.uid = Some(config.retained["123"].deployment_uid.clone());
    deployment.metadata.resource_version = Some("1".into());
    deployment
        .metadata
        .labels
        .as_mut()
        .unwrap()
        .remove("webdock.dev/mail-installation");
    (claim, config, storage, deployment)
}
#[test]
fn retained_execution_and_effective_security_extras_are_denied() {
    let (claim, config, _, deployment) = fixture();
    validate_workload(&deployment, &claim, &config).unwrap();
    let cases = [
        (
            "command",
            "/spec/template/spec/containers/0/command",
            json!(["sh", "-c", "exec stalwart --config /data/config.json"]),
        ),
        (
            "envFrom",
            "/spec/template/spec/containers/0/envFrom",
            json!([{"secretRef":{"name":"foreign"}}]),
        ),
        (
            "lifecycle",
            "/spec/template/spec/containers/0/lifecycle",
            json!({"postStart":{"exec":{"command":["sh", "-c", "true"]}}}),
        ),
        (
            "container user",
            "/spec/template/spec/containers/0/securityContext/runAsUser",
            json!(0),
        ),
        (
            "container seccomp",
            "/spec/template/spec/containers/0/securityContext/seccompProfile",
            json!({"type":"Unconfined"}),
        ),
        (
            "subpath mount",
            "/spec/template/spec/containers/0/volumeMounts/1/subPath",
            json!("foreign"),
        ),
        (
            "host port",
            "/spec/template/spec/containers/0/ports/0/hostPort",
            json!(8080),
        ),
        (
            "service account",
            "/spec/template/spec/serviceAccountName",
            json!("foreign"),
        ),
        (
            "DNS override",
            "/spec/template/spec/dnsConfig",
            json!({"nameservers":["10.0.0.2"]}),
        ),
        (
            "runtime class",
            "/spec/template/spec/runtimeClassName",
            json!("foreign"),
        ),
        (
            "node selector",
            "/spec/template/spec/nodeSelector",
            json!({"kubernetes.io/hostname":"foreign"}),
        ),
        (
            "memory override",
            "/spec/template/spec/containers/0/resources/limits/memory",
            json!("2Gi"),
        ),
    ];
    let mut accepted = Vec::new();
    for (name, pointer, value) in cases {
        let mut changed = deployment.clone();
        let (parent, field) = pointer.rsplit_once('/').unwrap();
        changed
            .data
            .pointer_mut(parent)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(field.into(), value);
        if validate_workload(&changed, &claim, &config).is_ok() {
            accepted.push(name);
        }
    }
    assert!(
        accepted.is_empty(),
        "unsupported retained execution fields accepted: {accepted:?}"
    );
}
struct Authority;
#[async_trait::async_trait]
impl MailAuthority for Authority {
    async fn guard(&self, _: &MailClaim) -> Result<()> {
        Ok(())
    }
}
#[tokio::test]
async fn invalid_retained_profile_is_denied_before_resolving_a_credential_destination() {
    let (claim, config, storage, mut deployment) = fixture();
    deployment.data["spec"]["template"]["spec"]["containers"][0]["image"] =
        json!("foreign.example.test/mail:mutable");
    let objects = mail_objects(&claim, &config, &storage, MailPhase::Normal).unwrap();
    let mut namespace = serde_json::to_value(objects.namespace).unwrap();
    namespace["metadata"]["uid"] = json!(config.retained["123"].namespace_uid);
    namespace["metadata"]["resourceVersion"] = json!("1");
    namespace["metadata"]["labels"]
        .as_object_mut()
        .unwrap()
        .remove("webdock.dev/mail-installation");
    let mut service = serde_json::to_value(objects.service).unwrap();
    service["metadata"]["uid"] = json!(config.retained["123"].service_uid);
    service["metadata"]["resourceVersion"] = json!("1");
    service["spec"]["clusterIP"] = json!("127.0.0.1");
    let reads = Arc::new(AtomicUsize::new(0));
    let observed = reads.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0_u8; 8192];
            let count = socket.read(&mut buffer).await.unwrap();
            let request = std::str::from_utf8(&buffer[..count]).unwrap();
            let path = request.split_whitespace().nth(1).unwrap();
            let body: Value = if path.ends_with("/namespaces/wd-mail-123") {
                namespace.clone()
            } else if path.ends_with("/deployments/app-123") {
                serde_json::to_value(&deployment).unwrap()
            } else if path.ends_with("/services/app-123") {
                observed.fetch_add(1, Ordering::SeqCst);
                service.clone()
            } else {
                panic!("unexpected API request {path}")
            };
            let body = serde_json::to_vec(&body).unwrap();
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).await.unwrap();
            socket.write_all(&body).await.unwrap();
        }
    });
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mail.json");
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let runtime = KubernetesMailRuntime {
        client: kube::Client::try_from(kube::Config::new(
            format!("http://{address}").parse().unwrap(),
        ))
        .unwrap(),
        storage: (
            StorageClient::new(directory.path().join("unreachable.sock")),
            storage,
        ),
        config,
        config_path: path,
    };
    let result = operations::Execution::new(&runtime, &claim, &Authority)
        .run()
        .await;
    server.abort();
    assert!(matches!(result, Err(ExecutionError::Ownership)));
    assert_eq!(
        reads.load(Ordering::SeqCst),
        0,
        "an invalid retained Deployment reached credential destination resolution"
    );
}

#[test]
fn reviewed_old_pod_profile_can_wait_during_native_rollout_without_becoming_ready() {
    let (claim, config, _, mut deployment) = fixture();
    deployment.metadata.labels.as_mut().unwrap().insert(
        "webdock.dev/mail-installation".into(),
        config.installation.clone(),
    );
    let mut pod = deployment.clone();
    pod.data = json!({"spec":deployment.data["spec"]["template"]["spec"]});
    pod.data["spec"]["containers"][0]["imagePullPolicy"] = json!("Never");
    pod.data["spec"]
        .as_object_mut()
        .unwrap()
        .remove("nodeSelector");
    super::lifecycle::validate_observed_pod(&deployment, &pod, &claim, &config).unwrap();
    assert!(!super::operations::contains(
        &pod.data["spec"],
        &deployment.data["spec"]["template"]["spec"]
    ));
    pod.data["spec"]["containers"][0]["command"] = json!(["sh", "-c", "true"]);
    assert!(super::lifecycle::validate_observed_pod(&deployment, &pod, &claim, &config).is_err());
}
