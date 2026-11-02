use k8s_openapi::api::{apps::v1::Deployment, core::v1::Namespace, networking::v1::NetworkPolicy};
use kube::{
    Api, Client,
    api::{DeleteParams, Patch, PatchParams, PostParams, Preconditions},
};
use serde_json::json;
use webdock_agent::mail::{MailAuthority, MailRuntime};
use webdock_agent_protocol::mail::{MailCheckpoint, MailClaim};
use webdock_supervisor::ExecutionError;
pub async fn verify(
    client: Client,
    runtime: &dyn MailRuntime,
    authority: &dyn MailAuthority,
    original: &MailClaim,
) {
    let mut claim = original.clone();
    claim.spec.operation_id = "7005".into();
    claim.spec.revision += 1;
    claim.lease.lease_token = uuid::Uuid::new_v4().to_string();
    claim.lease_until = chrono::Utc::now() + chrono::Duration::seconds(120);
    claim.checkpoint = None;
    let namespace = format!("wd-mail-{}", claim.spec.customer_id);
    let name = format!("app-{}", claim.spec.customer_id);
    let deployments = Api::<Deployment>::namespaced(client.clone(), &namespace);
    let before = deployments.get(&name).await.unwrap();
    claim.checkpoint = Some(MailCheckpoint {
        lease: claim.lease.clone(),
        runtime_id: uuid::Uuid::new_v4().to_string(),
        material_sha256: claim.spec.material_sha256.clone(),
    });
    assert!(matches!(
        runtime.execute(&claim, authority).await,
        Err(ExecutionError::Ownership)
    ));
    assert_eq!(
        deployments
            .get(&name)
            .await
            .unwrap()
            .metadata
            .resource_version,
        before.metadata.resource_version
    );
    claim.checkpoint = None;
    let policies = Api::<NetworkPolicy>::namespaced(client.clone(), &namespace);
    let policy: NetworkPolicy = serde_json::from_value(json!({"apiVersion":"networking.k8s.io/v1","kind":"NetworkPolicy","metadata":{"name":"fixture-foreign-ingress","namespace":namespace,"labels":{"webdock.dev/mail-test":"adversarial"}},"spec":{"podSelector":{},"policyTypes":["Ingress"],"ingress":[{}]}})).unwrap();
    assert!(
        policies
            .get_opt("fixture-foreign-ingress")
            .await
            .unwrap()
            .is_none()
    );
    let created = policies
        .create(&PostParams::default(), &policy)
        .await
        .unwrap();
    assert!(matches!(
        runtime.execute(&claim, authority).await,
        Err(ExecutionError::Ownership)
    ));
    assert_eq!(
        deployments
            .get(&name)
            .await
            .unwrap()
            .metadata
            .resource_version,
        before.metadata.resource_version
    );
    let observed = policies.get("fixture-foreign-ingress").await.unwrap();
    assert_eq!(observed.metadata.uid, created.metadata.uid);
    policies
        .delete(
            "fixture-foreign-ingress",
            &DeleteParams {
                preconditions: Some(Preconditions {
                    uid: created.metadata.uid,
                    resource_version: observed.metadata.resource_version,
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let namespaces = Api::<Namespace>::all(client);
    let before = namespaces.get(&namespace).await.unwrap();
    assert!(
        before
            .metadata
            .labels
            .as_ref()
            .unwrap()
            .contains_key("webdock.dev/mail-installation")
    );
    let current = namespaces.patch(&namespace, &PatchParams::apply("fixture-foreign-owner"), &Patch::Apply(json!({"apiVersion":"v1","kind":"Namespace","metadata":{"name":namespace,"uid":before.metadata.uid,"resourceVersion":before.metadata.resource_version,"labels":{"security.webdock.dev/ready":"true"}}}))).await.unwrap();
    assert!(matches!(
        runtime.execute(&claim, authority).await,
        Err(ExecutionError::Ownership | ExecutionError::Conflict)
    ));
    assert_eq!(
        namespaces
            .get(&namespace)
            .await
            .unwrap()
            .metadata
            .resource_version,
        current.metadata.resource_version
    );
}

pub async fn denied_suspended_execution_override(
    client: Client,
    runtime: &dyn MailRuntime,
    authority: &dyn MailAuthority,
    claim: &MailClaim,
) {
    let namespace = format!("wd-mail-{}", claim.spec.customer_id);
    let name = format!("app-{}", claim.spec.customer_id);
    let deployments = Api::<Deployment>::namespaced(client.clone(), &namespace);
    let before = deployments.get(&name).await.unwrap();
    assert_eq!(before.spec.as_ref().unwrap().replicas, Some(0));
    assert!(
        before
            .metadata
            .labels
            .as_ref()
            .unwrap()
            .contains_key("webdock.dev/mail-installation")
    );
    let changed = deployments.patch(&name, &PatchParams { field_manager: Some("fixture-execution-override".into()), ..Default::default() }, &Patch::Strategic(json!({"metadata":{"uid":before.metadata.uid,"resourceVersion":before.metadata.resource_version},"spec":{"template":{"spec":{"containers":[{"name":"app","command":["/usr/local/bin/stalwart"]}]}}}}))).await.unwrap();
    assert!(matches!(
        runtime.execute(claim, authority).await,
        Err(ExecutionError::Ownership)
    ));
    let observed = deployments.get(&name).await.unwrap();
    assert_eq!(observed.metadata.uid, changed.metadata.uid);
    assert_eq!(observed.spec, changed.spec);
    assert_eq!(observed.spec.as_ref().unwrap().replicas, Some(0));
    assert!(
        Api::<k8s_openapi::api::core::v1::Pod>::namespaced(client, &namespace)
            .list(&kube::api::ListParams::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    deployments.patch(&name, &PatchParams { field_manager: Some("fixture-execution-override".into()), ..Default::default() }, &Patch::Strategic(json!({"metadata":{"uid":observed.metadata.uid,"resourceVersion":observed.metadata.resource_version},"spec":{"template":{"spec":{"containers":[{"name":"app","command":null}]}}}}))).await.unwrap();
}
