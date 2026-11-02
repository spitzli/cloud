use super::configuration_fixture::{ConfigurationAuthority, configuration_claim};
use k8s_openapi::api::{
    apps::v1::Deployment,
    core::v1::{PersistentVolumeClaim, Pod, Service},
};
use kube::{Api, Client, api::ListParams};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;
use webdock_agent::mail::{configuration::ConfigurationRuntime, kubernetes::KubernetesMailRuntime};
use webdock_agent_protocol::{
    mail::{MailClaim, MailProof},
    mail_configuration::*,
};
use webdock_supervisor::ExecutionError;

fn authority() -> ConfigurationAuthority {
    ConfigurationAuthority {
        allowed: AtomicBool::new(true),
        drop_applied: AtomicBool::new(true),
        checkpoints: Mutex::new(Vec::new()),
    }
}
async fn session(
    client: &reqwest::Client,
    origin: &str,
    credential: &ConfigurationCredential,
) -> reqwest::Response {
    client
        .get(origin)
        .basic_auth(&credential.username, Some(&credential.password))
        .send()
        .await
        .unwrap()
}
pub async fn verify(
    client: &Client,
    runtime: &KubernetesMailRuntime,
    lifecycle: &MailClaim,
    instance: &MailProof,
) -> Value {
    let namespace = &instance.namespace;
    let name = format!("app-{}", lifecycle.spec.customer_id);
    let deployments = Api::<Deployment>::namespaced(client.clone(), namespace);
    let pods = Api::<Pod>::namespaced(client.clone(), namespace);
    let claims = Api::<PersistentVolumeClaim>::namespaced(client.clone(), namespace);
    let before = deployments.get(&name).await.unwrap();
    let before_pods = pods.list(&ListParams::default()).await.unwrap().items;
    let before_claim = claims.get(&format!("{name}-data")).await.unwrap();
    assert_eq!(before_pods.len(), 1);
    let service = Api::<Service>::namespaced(client.clone(), namespace)
        .get(&name)
        .await
        .unwrap();
    let ip: std::net::Ipv4Addr = service.spec.unwrap().cluster_ip.unwrap().parse().unwrap();
    assert!(ip.is_private());
    let origin = format!("http://{ip}:8080/jmap/session");
    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    let mut claim = configuration_claim(lifecycle, instance);
    let authority = authority();
    assert!(matches!(
        runtime.configure(&claim, &authority).await,
        Err(ExecutionError::Journal)
    ));
    claim.checkpoints = authority.checkpoints.lock().await.clone();
    assert!(claim.checkpoints.iter().any(|checkpoint| matches!(
        checkpoint.phase,
        ConfigurationCheckpointPhase::Applied { .. }
    )));
    let proof = runtime.configure(&claim, &authority).await.unwrap();
    assert_eq!(proof.runtime_id, instance.runtime_id);
    assert_eq!(proof.mailboxes.len(), 2);
    let identity = proof.identities["7004"].clone().unwrap();
    let credential = claim.material.identities["7004"].clone();
    let initial: Value = session(&http, &origin, &credential)
        .await
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(initial["accounts"].get(&proof.mailboxes["7002"]).is_some());
    assert!(initial["accounts"].get(&proof.mailboxes["7003"]).is_some());
    authority.allowed.store(false, Ordering::SeqCst);
    assert!(matches!(
        runtime.configure(&claim, &authority).await,
        Err(ExecutionError::Denied)
    ));
    authority.allowed.store(true, Ordering::SeqCst);
    claim.spec.mode = ConfigurationMode::Restrict;
    claim.spec.operation_id = "7006".into();
    claim.spec.revision = "2".into();
    claim.spec.snapshot.domains.clear();
    claim
        .spec
        .snapshot
        .mailboxes
        .retain(|mailbox| mailbox.id == "7002");
    claim.spec.snapshot.mailboxes[0].provider_id = Some(proof.mailboxes["7002"].clone());
    claim.spec.snapshot.identities[0].provider_id = Some(identity.clone());
    claim.spec.snapshot.identities[0].mailbox_ids = vec!["7002".into()];
    claim.spec.snapshot.grants = vec![ConfigurationGrant {
        mailbox_id: "7003".into(),
        subject: "42".into(),
        revision: "2".into(),
        enabled: false,
    }];
    claim.material.identities.clear();
    claim.spec.material_sha256 = claim.material.fingerprint().unwrap();
    claim.lease.lease_token = uuid::Uuid::new_v4().to_string();
    claim.lease_until = chrono::Utc::now() + chrono::Duration::seconds(120);
    claim.checkpoints.clear();
    authority.checkpoints.lock().await.clear();
    let restricted = runtime.configure(&claim, &authority).await.unwrap();
    assert!(restricted.domains.is_empty() && restricted.dkim.is_empty());
    assert!(
        authority
            .checkpoints
            .lock()
            .await
            .iter()
            .all(|checkpoint| match &checkpoint.phase {
                ConfigurationCheckpointPhase::Bound {} => true,
                ConfigurationCheckpointPhase::Handoff { resource, .. }
                | ConfigurationCheckpointPhase::Applied { resource, .. } =>
                    resource.kind == ConfigurationResourceKind::Identity,
            })
    );
    let after: Value = session(&http, &origin, &credential)
        .await
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(after["accounts"].get(&proof.mailboxes["7002"]).is_some());
    assert!(after["accounts"].get(&proof.mailboxes["7003"]).is_none());
    claim.spec.operation_id = "7007".into();
    claim.spec.revision = "3".into();
    claim.spec.snapshot.mailboxes.clear();
    claim.spec.snapshot.identities[0].enabled = false;
    claim.spec.snapshot.identities[0].mailbox_ids.clear();
    claim.spec.snapshot.grants[0].mailbox_id = "7002".into();
    claim.spec.snapshot.grants[0].revision = "3".into();
    claim.lease.lease_token = uuid::Uuid::new_v4().to_string();
    claim.lease_until = chrono::Utc::now() + chrono::Duration::seconds(120);
    claim.checkpoints.clear();
    authority.checkpoints.lock().await.clear();
    runtime.configure(&claim, &authority).await.unwrap();
    assert!(matches!(
        session(&http, &origin, &credential).await.status(),
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
    ));
    let after = deployments.get(&name).await.unwrap();
    assert_eq!(after.metadata.uid, before.metadata.uid);
    assert_eq!(after.metadata.generation, before.metadata.generation);
    assert_eq!(after.spec, before.spec);
    let after_pods = pods.list(&ListParams::default()).await.unwrap().items;
    assert_eq!(after_pods.len(), 1);
    assert_eq!(after_pods[0].metadata.uid, before_pods[0].metadata.uid);
    let after_claim = claims.get(&format!("{name}-data")).await.unwrap();
    assert_eq!(after_claim.metadata.uid, before_claim.metadata.uid);
    assert_eq!(after_claim.spec, before_claim.spec);
    json!({"customerID":lifecycle.spec.customer_id,"namespace":namespace,"deploymentUID":proof.runtime_id,"podUID":after_pods[0].metadata.uid,"claimUID":after_claim.metadata.uid,"configuration":true,"checkpointResponseLoss":true,"domainProviderID":proof.domains["7001"],"mailboxProviderIDs":proof.mailboxes,"identityProviderID":identity,"dkim":proof.dkim,"stableRuntime":true,"partialAndFullRestriction":true,"credentialRetained":true,"guardDenied":true})
}
pub async fn deny_suspended(
    client: &Client,
    runtime: &KubernetesMailRuntime,
    lifecycle: &MailClaim,
    instance: &MailProof,
) {
    let mut claim = configuration_claim(lifecycle, instance);
    claim.spec.operation_id = "7009".into();
    let authority = authority();
    assert!(matches!(
        runtime.configure(&claim, &authority).await,
        Err(ExecutionError::Ownership)
    ));
    assert!(authority.checkpoints.lock().await.is_empty());
    let deployment = Api::<Deployment>::namespaced(client.clone(), &instance.namespace)
        .get(&format!("app-{}", lifecycle.spec.customer_id))
        .await
        .unwrap();
    assert_eq!(deployment.spec.unwrap().replicas, Some(0));
    assert!(
        Api::<Pod>::namespaced(client.clone(), &instance.namespace)
            .list(&ListParams::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
}
