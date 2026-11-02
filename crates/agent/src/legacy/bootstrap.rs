use super::*;
use k8s_openapi::api::{
    core::v1::{LimitRange, ServiceAccount},
    networking::v1::NetworkPolicy,
};
use kube::core::{ApiResource, DynamicObject};
use serde_json::json;

pub fn new_workload(command: &Command) -> Result<(Deployment, Service)> {
    command.validate()?;
    let spec = &command.spec;
    if command.expected_runtime_id.is_some() || command.action != Action::Apply {
        return Err(ExecutionError::Ownership);
    }
    let labels = owner_labels(spec, true)?;
    let annotations = json!({"webdock.dev/revision":spec.revision.to_string(),"webdock.dev/operation":command.operation_id});
    let mut volumes =
        vec![json!({"name":"tmp","emptyDir":{"sizeLimit":spec.ephemeral_bytes.to_string()}})];
    let mut mounts = vec![json!({"name":"tmp","mountPath":"/tmp"})];
    if spec.volume.is_some() {
        volumes.push(json!({"name":"data","persistentVolumeClaim":{"claimName":format!("{}-data",name(spec))}}));
        mounts.push(json!({"name":"data","mountPath":"/data"}));
    }
    let mut pod_labels = labels.clone();
    pod_labels.insert("security.webdock.dev/ingress".into(), "true".into());
    let metadata = json!({"name":name(spec),"namespace":namespace(spec),"labels":labels,"annotations":annotations});
    let health = probe(spec)?;
    let deployment = json!({"apiVersion":"apps/v1","kind":"Deployment","metadata":metadata,"spec":{"replicas":spec.replicas,"strategy":{"type":"Recreate"},"revisionHistoryLimit":2,"selector":{"matchLabels":{"webdock.dev/app-id":spec.resource_id}},"template":{"metadata":{"labels":pod_labels,"annotations":annotations},"spec":{"automountServiceAccountToken":false,"serviceAccountName":"default","terminationGracePeriodSeconds":15,"securityContext":{"runAsNonRoot":true,"runAsUser":65532,"runAsGroup":65532,"fsGroup":65532,"seccompProfile":{"type":"RuntimeDefault"}},"containers":[{"name":"app","image":spec.image,"imagePullPolicy":"IfNotPresent","args":spec.arguments,"ports":[{"containerPort":spec.port}],"securityContext":{"allowPrivilegeEscalation":false,"privileged":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}},"resources":resources(spec),"readinessProbe":health,"livenessProbe":health,"startupProbe":Probe{failure_threshold:Some(20),..health},"volumeMounts":mounts}],"volumes":volumes}}}});
    let service = json!({"apiVersion":"v1","kind":"Service","metadata":metadata,"spec":{"type":"ClusterIP","selector":{"webdock.dev/app-id":spec.resource_id},"ports":[{"name":"http","port":spec.port,"targetPort":spec.port}]}});
    Ok((
        serde_json::from_value(deployment).map_err(|_| ExecutionError::Runtime)?,
        serde_json::from_value(service).map_err(|_| ExecutionError::Runtime)?,
    ))
}
async fn namespaced_object(
    client: Client,
    spec: &RuntimeSpec,
    kind: &str,
    name: &str,
    body: serde_json::Value,
    guard: &LeaseGuard,
) -> Result<()> {
    let resource = match kind {
        "LimitRange" => ApiResource::erase::<LimitRange>(&()),
        "ServiceAccount" => ApiResource::erase::<ServiceAccount>(&()),
        "NetworkPolicy" => ApiResource::erase::<NetworkPolicy>(&()),
        _ => return Err(ExecutionError::Invalid("namespace object")),
    };
    let mut value = json!({"apiVersion":resource.api_version,"kind":kind,"metadata":{"name":name,"namespace":namespace(spec),"labels":owner_labels(spec,false)?}});
    if kind == "ServiceAccount" {
        value["automountServiceAccountToken"] = false.into();
    } else {
        value["spec"] = body;
    }
    let api: Api<DynamicObject> = Api::namespaced_with(client, &namespace(spec), &resource);
    let current = api.get_opt(name).await.map_err(api_failure)?;
    if let Some(current) = &current {
        if kind != "ServiceAccount"
            || current
                .metadata
                .labels
                .as_ref()
                .is_some_and(|l| l.contains_key("security.webdock.dev/managed-by"))
        {
            owner(&current.metadata, spec, false)?;
        }
        value["metadata"]["resourceVersion"] = json!(current.metadata.resource_version);
        value["metadata"]["uid"] = json!(current.metadata.uid);
    }
    let desired = serde_json::from_value(value).map_err(|_| ExecutionError::Runtime)?;
    let params = PostParams {
        field_manager: Some("webdock".into()),
        ..Default::default()
    };
    guard.check().await?;
    if current.is_some() {
        api.replace(name, &params, &desired)
            .await
            .map_err(api_failure)?;
    } else {
        api.create(&params, &desired).await.map_err(api_failure)?;
    }
    Ok(())
}
pub(super) async fn namespace_ready(
    client: Client,
    command: &Command,
    guard: &LeaseGuard,
) -> Result<Namespace> {
    let spec = &command.spec;
    let api: Api<Namespace> = Api::all(client.clone());
    let current = api.get_opt(&namespace(spec)).await.map_err(api_failure)?;
    let mut ns = if let Some(ns) = current {
        owner(&ns.metadata, spec, false)?;
        ns
    } else {
        let mut labels = owner_labels(spec, false)?;
        labels.extend([
            ("security.webdock.dev/ready".into(), "false".into()),
            (
                "pod-security.kubernetes.io/enforce".into(),
                "restricted".into(),
            ),
            (
                "pod-security.kubernetes.io/enforce-version".into(),
                "v1.36".into(),
            ),
        ]);
        guard.check().await?;
        api.create(
            &PostParams::default(),
            &Namespace {
                metadata: ObjectMeta {
                    name: Some(namespace(spec)),
                    labels: Some(labels),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .await
        .map_err(api_failure)?
    };
    if spec
        .hosting_binding
        .as_ref()
        .is_some_and(|b| b.public_route.is_some())
    {
        ns.metadata
            .labels
            .get_or_insert_default()
            .insert("webdock.dev/certificate-budget".into(), "1".into());
        namespaced_object(client.clone(),spec,"NetworkPolicy","webdock-acme-solver",json!({"podSelector":{"matchLabels":{"acme.cert-manager.io/http01-solver":"true"}},"policyTypes":["Ingress"],"ingress":[{"from":[{"namespaceSelector":{"matchLabels":{"kubernetes.io/metadata.name":"kube-system"}},"podSelector":{"matchLabels":{"app.kubernetes.io/name":"traefik"}}}],"ports":[{"protocol":"TCP","port":8089}]}]}),guard).await?;
    }
    quota(
        client.clone(),
        spec,
        &spec
            .hosting_binding
            .as_ref()
            .ok_or(ExecutionError::Ownership)?
            .quota,
        &ns,
        guard,
    )
    .await?;
    namespaced_object(client.clone(),spec,"LimitRange","webdock-defaults",json!({"limits":[{"type":"Container","defaultRequest":{"cpu":"100m","memory":"128Mi","ephemeral-storage":"128Mi"},"default":{"cpu":"100m","memory":"128Mi","ephemeral-storage":"128Mi"}}]}),guard).await?;
    namespaced_object(
        client.clone(),
        spec,
        "ServiceAccount",
        "default",
        serde_json::Value::Null,
        guard,
    )
    .await?;
    for (name, body) in [
        (
            "webdock-deny",
            json!({"podSelector":{},"policyTypes":["Ingress","Egress"]}),
        ),
        (
            "webdock-internal",
            json!({"podSelector":{},"policyTypes":["Ingress","Egress"],"ingress":[{"from":[{"podSelector":{}}]}],"egress":[{"to":[{"podSelector":{}}]}]}),
        ),
        (
            "webdock-dns",
            json!({"podSelector":{},"policyTypes":["Egress"],"egress":[{"to":[{"namespaceSelector":{"matchLabels":{"kubernetes.io/metadata.name":"kube-system"}},"podSelector":{"matchLabels":{"k8s-app":"kube-dns"}}}],"ports":[{"protocol":"UDP","port":53},{"protocol":"TCP","port":53}]}]}),
        ),
        (
            "webdock-ingress",
            json!({"podSelector":{"matchLabels":{"security.webdock.dev/ingress":"true"}},"policyTypes":["Ingress"],"ingress":[{"from":[{"namespaceSelector":{"matchLabels":{"kubernetes.io/metadata.name":"kube-system"}},"podSelector":{"matchLabels":{"app.kubernetes.io/name":"traefik"}}}]}]}),
        ),
    ] {
        namespaced_object(client.clone(), spec, "NetworkPolicy", name, body, guard).await?;
    }
    ns.metadata
        .labels
        .get_or_insert_default()
        .insert("security.webdock.dev/ready".into(), "true".into());
    ns.status = None;
    guard.check().await?;
    api.replace(&namespace(spec), &PostParams::default(), &ns)
        .await
        .map_err(|_| ExecutionError::Runtime)
}
