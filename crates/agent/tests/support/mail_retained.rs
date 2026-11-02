use kube::{
    Api, Client,
    api::{ApiResource, DynamicObject, GroupVersionKind, Patch, PatchParams},
};
use serde_json::json;
use std::{collections::BTreeMap, os::unix::fs::PermissionsExt, path::Path};
use webdock_agent::{
    mail::{
        MailAuthority, MailRuntime,
        kubernetes::{
            KubernetesMailConfig, KubernetesMailRuntime, MailPhase, RetainedBinding, mail_objects,
        },
    },
    storage::{StorageClient, StorageConfig},
};
use webdock_agent_protocol::mail::MailClaim;
use webdock_supervisor::ExecutionError;

pub async fn adopt(
    client: Client,
    storage: (StorageClient, StorageConfig),
    path: &Path,
    authority: &dyn MailAuthority,
    claim: &mut MailClaim,
) -> KubernetesMailRuntime {
    let namespace = format!("wd-mail-{}", claim.spec.customer_id);
    let app = format!("app-{}", claim.spec.customer_id);
    let mut ids = BTreeMap::new();
    let config = KubernetesMailConfig::load(path).unwrap();
    let resources = [
        ("", "Namespace", namespace.clone(), false),
        ("apps", "Deployment", app.clone(), true),
        ("", "Service", app.clone(), true),
        (
            "",
            "PersistentVolume",
            format!("wd-{}-data", claim.spec.customer_id),
            false,
        ),
        ("", "PersistentVolumeClaim", format!("{app}-data"), true),
    ];
    for (group, kind, name, namespaced) in resources {
        let resource = ApiResource::from_gvk(&GroupVersionKind::gvk(group, "v1", kind));
        let api: Api<DynamicObject> = if namespaced {
            Api::namespaced_with(client.clone(), &namespace, &resource)
        } else {
            Api::all_with(client.clone(), &resource)
        };
        let observed = api.get(&name).await.unwrap();
        assert_eq!(
            observed.metadata.labels.as_ref().unwrap()["webdock.dev/mail-installation"],
            config.installation
        );
        ids.insert(kind, observed.metadata.uid.clone().unwrap());
        // Construct the retained legacy metadata boundary without replacing workloads/storage.
        // This ordinary conditional metadata patch never writes API-owned managedFields.
        api.patch(&name, &PatchParams::default(), &Patch::Merge(json!({"metadata":{"uid":observed.metadata.uid,"resourceVersion":observed.metadata.resource_version,"labels":{"webdock.dev/mail-installation":null},"annotations":{"webdock.dev/mail-profile":null,"webdock.dev/mail-creation-operation":null,"webdock.dev/mail-creation-generation":null,"webdock.dev/mail-creation-revision":null}}}))).await.unwrap();
    }
    historical_workload(
        client.clone(),
        claim,
        &config,
        &storage.1,
        &ids["Deployment"],
    )
    .await;
    claim.spec.operation_id = "7004".into();
    claim.spec.revision += 1;
    claim.lease.lease_token = uuid::Uuid::new_v4().to_string();
    claim.lease_until = chrono::Utc::now() + chrono::Duration::seconds(120);
    claim.checkpoint = None;
    let unbound =
        KubernetesMailRuntime::open(client.clone(), storage.clone(), path.to_path_buf()).unwrap();
    assert!(matches!(
        unbound.execute(claim, authority).await,
        Err(ExecutionError::Ownership)
    ));
    let mut reviewed = config;
    reviewed.retained.insert(
        claim.spec.customer_id.clone(),
        RetainedBinding {
            namespace_uid: ids["Namespace"].clone(),
            deployment_uid: ids["Deployment"].clone(),
            service_uid: ids["Service"].clone(),
            volume_uid: ids["PersistentVolume"].clone(),
            claim_uid: ids["PersistentVolumeClaim"].clone(),
            bootstrap_secret: None,
            policies: BTreeMap::new(),
        },
    );
    std::fs::write(path, serde_json::to_vec(&reviewed).unwrap()).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let runtime = KubernetesMailRuntime::open(client.clone(), storage, path.to_path_buf()).unwrap();
    refused_service_protocol(client.clone(), &runtime, authority, claim, &ids["Service"]).await;
    let proof = runtime.execute(claim, authority).await.unwrap();
    assert!(
        proof.running && proof.recovery_disabled && proof.submission_certificate_sha256.is_some()
    );
    assert_eq!(proof.runtime_id, ids["Deployment"]);
    let native =
        Api::<k8s_openapi::api::apps::v1::Deployment>::namespaced(client.clone(), &namespace)
            .get(&app)
            .await
            .unwrap();
    let template = native
        .spec
        .as_ref()
        .unwrap()
        .template
        .spec
        .as_ref()
        .unwrap();
    assert_eq!(
        template.containers[0].image_pull_policy.as_deref(),
        Some("IfNotPresent")
    );
    assert_eq!(
        template.node_selector.as_ref().unwrap()["kubernetes.io/hostname"],
        reviewed.node
    );
    let pods = Api::<k8s_openapi::api::core::v1::Pod>::namespaced(client, &namespace)
        .list(&kube::api::ListParams::default())
        .await
        .unwrap();
    assert_eq!(pods.items.len(), 1);
    let pod = pods.items[0].spec.as_ref().unwrap();
    assert_eq!(
        pod.containers[0].image_pull_policy.as_deref(),
        Some("IfNotPresent")
    );
    assert_eq!(
        pod.node_selector.as_ref().unwrap()["kubernetes.io/hostname"],
        reviewed.node
    );
    runtime
}

async fn historical_workload(
    client: Client,
    claim: &MailClaim,
    config: &KubernetesMailConfig,
    storage: &StorageConfig,
    uid: &str,
) {
    use k8s_openapi::api::{apps::v1::Deployment, core::v1::Pod};
    let namespace = format!("wd-mail-{}", claim.spec.customer_id);
    let name = format!("app-{}", claim.spec.customer_id);
    let api = Api::<Deployment>::namespaced(client.clone(), &namespace);
    let observed = api.get(&name).await.unwrap();
    assert_eq!(observed.metadata.uid.as_deref(), Some(uid));
    let patched = api.patch(&name, &PatchParams::default(), &Patch::Merge(json!({"metadata":{"uid":uid,"resourceVersion":observed.metadata.resource_version},"spec":{"template":{"spec":{"nodeSelector":null}}}}))).await.unwrap();
    let mut historical = mail_objects(claim, config, storage, MailPhase::Normal)
        .unwrap()
        .deployment;
    historical.metadata.uid = Some(uid.into());
    historical.metadata.resource_version = patched.metadata.resource_version;
    historical
        .metadata
        .labels
        .as_mut()
        .unwrap()
        .remove("webdock.dev/mail-installation");
    historical
        .metadata
        .annotations
        .as_mut()
        .unwrap()
        .remove("webdock.dev/mail-profile");
    let template = &mut historical.spec.as_mut().unwrap().template;
    template.metadata.as_mut().unwrap().annotations = observed
        .spec
        .as_ref()
        .unwrap()
        .template
        .metadata
        .as_ref()
        .unwrap()
        .annotations
        .clone();
    template
        .metadata
        .as_mut()
        .unwrap()
        .annotations
        .as_mut()
        .unwrap()
        .remove("webdock.dev/submission-config");
    template.spec.as_mut().unwrap().node_selector = None;
    template.spec.as_mut().unwrap().containers[0].image_pull_policy = Some("Never".into());
    // The owned fixture reconstructs the historical webdock Apply profile. Runtime adoption itself never forces legacy fields.
    api.patch(
        &name,
        &PatchParams::apply("webdock").force(),
        &Patch::Apply(&historical),
    )
    .await
    .unwrap();
    let pods = Api::<Pod>::namespaced(client, &namespace);
    let mut ready = false;
    for _ in 0..300 {
        let deployment = api.get(&name).await.unwrap();
        assert_eq!(deployment.metadata.uid.as_deref(), Some(uid));
        let current = pods
            .list(&kube::api::ListParams::default())
            .await
            .unwrap()
            .items;
        ready = deployment.status.as_ref().is_some_and(|status| {
            status.observed_generation >= deployment.metadata.generation
                && status.available_replicas == Some(1)
        }) && current.len() == 1
            && current[0].metadata.deletion_timestamp.is_none()
            && current[0].spec.as_ref().unwrap().containers[0]
                .image_pull_policy
                .as_deref()
                == Some("Never")
            && current[0].spec.as_ref().unwrap().node_selector.is_none()
            && current[0]
                .status
                .as_ref()
                .and_then(|status| status.conditions.as_ref())
                .is_some_and(|conditions| {
                    conditions
                        .iter()
                        .any(|c| c.type_ == "Ready" && c.status == "True")
                });
        if ready {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    assert!(
        ready,
        "historical Never/no-nodeSelector Pod must be running before migration"
    );
}

async fn refused_service_protocol(
    client: Client,
    runtime: &KubernetesMailRuntime,
    authority: &dyn MailAuthority,
    claim: &MailClaim,
    uid: &str,
) {
    let namespace = format!("wd-mail-{}", claim.spec.customer_id);
    let name = format!("app-{}", claim.spec.customer_id);
    let api = Api::<k8s_openapi::api::core::v1::Service>::namespaced(client, &namespace);
    let before = api.get(&name).await.unwrap();
    assert_eq!(before.metadata.uid.as_deref(), Some(uid));
    let mut ports = before.spec.as_ref().unwrap().ports.clone().unwrap();
    let smtp = ports.iter_mut().find(|port| port.port == 2525).unwrap();
    assert_eq!(smtp.protocol.as_deref(), Some("TCP"));
    smtp.protocol = Some("UDP".into());
    let changed = api.patch(&name, &PatchParams::default(), &Patch::Merge(json!({"metadata":{"uid":uid,"resourceVersion":before.metadata.resource_version},"spec":{"ports":ports}}))).await.unwrap();
    assert!(matches!(
        runtime.execute(claim, authority).await,
        Err(ExecutionError::Ownership)
    ));
    let current = api.get(&name).await.unwrap();
    assert_eq!(current.metadata.uid.as_deref(), Some(uid));
    assert_eq!(current.spec, changed.spec);
    api.patch(&name, &PatchParams::default(), &Patch::Merge(json!({"metadata":{"uid":uid,"resourceVersion":current.metadata.resource_version},"spec":{"ports":before.spec.unwrap().ports}}))).await.unwrap();
}
