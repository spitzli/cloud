//! Native HPA ownership. Never accepts customer-provided Kubernetes objects.
use k8s_openapi::api::{apps::v1::Deployment, autoscaling::v2::HorizontalPodAutoscaler};
use kube::{
    Api, Client,
    api::{DeleteParams, Patch, PatchParams, PostParams, Preconditions},
};
use serde_json::{Value, json};
use spitzli_supervisor::{AutoscalingObservation, Command, LeaseGuard, ObservedState};
use spitzli_supervisor::{ExecutionError, Result, RuntimeSpec};
use std::time::Duration;

fn api_error(_: kube::Error) -> ExecutionError {
    ExecutionError::Runtime
}

/// Opt-in is set only after the operator qualifies metrics and HPA lifecycle.
pub fn enabled() -> bool {
    std::env::var("SPITZLI_AGENT_AUTOSCALING_ENABLED").as_deref() == Ok("true")
}

pub fn identity(spec: &RuntimeSpec) -> Result<(String, String, Value)> {
    if spec.hosting_binding.is_some() {
        Ok((
            crate::legacy::namespace(spec),
            crate::legacy::name(spec),
            json!(crate::legacy::owner_labels(spec, true)?),
        ))
    } else {
        Ok((spec.namespace(), spec.name(), json!(spec.labels())))
    }
}

pub fn manifest(spec: &RuntimeSpec, deployment_uid: &str) -> Result<Value> {
    spec.validate()?;
    if deployment_uid.is_empty() {
        return Err(ExecutionError::Ownership);
    }
    let policy = spec
        .autoscaling
        .as_ref()
        .ok_or(ExecutionError::Invalid("missing scaling policy"))?;
    let (namespace, name, labels) = identity(spec)?;
    Ok(json!({
        "apiVersion":"autoscaling/v2", "kind":"HorizontalPodAutoscaler",
        "metadata":{"name":name,"namespace":namespace,"labels":labels,
            "annotations":{"spitzli.cloud/scaling-revision":spec.revision.to_string()},
            "ownerReferences":[{"apiVersion":"apps/v1","kind":"Deployment","name":name,
                "uid":deployment_uid,"controller":true,"blockOwnerDeletion":false}]},
        "spec":{"scaleTargetRef":{"apiVersion":"apps/v1","kind":"Deployment","name":name},
            "minReplicas":policy.min_replicas,"maxReplicas":policy.max_replicas,
            "metrics":[{"type":"Resource","resource":{"name":"cpu","target":{
                "type":"Utilization","averageUtilization":policy.target_cpu_utilization_percent}}}],
            "behavior":{"scaleUp":{"stabilizationWindowSeconds":0,"selectPolicy":"Max",
                "policies":[{"type":"Percent","value":100,"periodSeconds":60}]},
                "scaleDown":{"stabilizationWindowSeconds":300,"selectPolicy":"Min",
                "policies":[{"type":"Percent","value":50,"periodSeconds":60}]}}}
    }))
}

pub fn verify(spec: &RuntimeSpec, deployment_uid: &str, object: &Value, exact: bool) -> Result<()> {
    let (namespace, name, labels) = identity(spec)?;
    let owner = json!([{"apiVersion":"apps/v1","kind":"Deployment","name":name,
        "uid":deployment_uid,"controller":true,"blockOwnerDeletion":false}]);
    let revision = object["metadata"]["annotations"]["spitzli.cloud/scaling-revision"]
        .as_str()
        .and_then(|value| value.parse::<u64>().ok());
    if deployment_uid.is_empty()
        || object["apiVersion"] != "autoscaling/v2"
        || object["kind"] != "HorizontalPodAutoscaler"
        || object["metadata"]["name"] != name
        || object["metadata"]["namespace"] != namespace
        || object["metadata"]["labels"] != labels
        || object["metadata"]["ownerReferences"] != owner
        || object["spec"]["scaleTargetRef"]
            != json!({"apiVersion":"apps/v1","kind":"Deployment","name":name})
        || revision.is_none_or(|r| r > spec.revision || r == 0)
    {
        return Err(ExecutionError::Ownership);
    }
    if exact
        && (object["spec"] != manifest(spec, deployment_uid)?["spec"]
            || revision != Some(spec.revision))
    {
        return Err(ExecutionError::Ownership);
    }
    Ok(())
}

pub async fn remove(client: Client, command: &Command, guard: &LeaseGuard) -> Result<()> {
    let spec = &command.spec;
    let (namespace, name, _) = identity(spec)?;
    let api: Api<HorizontalPodAutoscaler> = Api::namespaced(client.clone(), &namespace);
    let Some(existing) = api.get_opt(&name).await.map_err(api_error)? else {
        return Ok(());
    };
    let deployments: Api<Deployment> = Api::namespaced(client, &namespace);
    let deployment = deployments.get(&name).await.map_err(api_error)?;
    let uid = deployment
        .metadata
        .uid
        .as_deref()
        .ok_or(ExecutionError::Ownership)?;
    if command
        .expected_runtime_id
        .as_deref()
        .is_some_and(|expected| expected != uid)
    {
        return Err(ExecutionError::Ownership);
    }
    verify(spec, uid, &json!(existing), false)?;
    if existing.metadata.uid.is_none() || existing.metadata.resource_version.is_none() {
        return Err(ExecutionError::Ownership);
    }
    guard.check_for(command).await?;
    api.delete(
        &name,
        &DeleteParams {
            preconditions: Some(Preconditions {
                uid: existing.metadata.uid.clone(),
                resource_version: existing.metadata.resource_version.clone(),
            }),
            ..Default::default()
        },
    )
    .await
    .map_err(api_error)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        guard.check_for(command).await?;
        if api.get_opt(&name).await.map_err(api_error)?.is_none() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(ExecutionError::Runtime);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

pub async fn apply(client: Client, command: &Command, guard: &LeaseGuard, uid: &str) -> Result<()> {
    if !enabled() {
        return Err(ExecutionError::Unsupported("qualified automatic scaling"));
    }
    let spec = &command.spec;
    let (namespace, name, _) = identity(spec)?;
    guard.check_for(command).await?;
    let deployments: Api<Deployment> = Api::namespaced(client.clone(), &namespace);
    let deployment = deployments.get(&name).await.map_err(api_error)?;
    if deployment.metadata.uid.as_deref() != Some(uid) {
        return Err(ExecutionError::Ownership);
    }
    if deployment.spec.as_ref().and_then(|s| s.replicas) == Some(0) {
        // HPA deliberately does not reactivate a zero target: only an admitted resume may do so.
        let policy = spec
            .autoscaling
            .as_ref()
            .ok_or(ExecutionError::Invalid("missing scaling policy"))?;
        let rv = deployment
            .metadata
            .resource_version
            .as_deref()
            .ok_or(ExecutionError::Ownership)?;
        guard.check_for(command).await?;
        deployments.patch(&name, &PatchParams::default(), &Patch::Merge(json!({
            "metadata":{"uid":uid,"resourceVersion":rv},"spec":{"replicas":policy.min_replicas}
        }))).await.map_err(api_error)?;
    }
    let api: Api<HorizontalPodAutoscaler> = Api::namespaced(client, &namespace);
    let mut desired: HorizontalPodAutoscaler = serde_json::from_value(manifest(spec, uid)?)
        .map_err(|_| ExecutionError::Invalid("HPA manifest"))?;
    let params = PostParams {
        field_manager: Some("spitzli-autoscaling".into()),
        ..Default::default()
    };
    if let Some(existing) = api.get_opt(&name).await.map_err(api_error)? {
        verify(spec, uid, &json!(existing), false)?;
        // Even identical policies advance the CAS fence against delayed older writes.
        desired.metadata.resource_version = existing.metadata.resource_version;
        desired.metadata.uid = existing.metadata.uid;
        guard.check_for(command).await?;
        api.replace(&name, &params, &desired)
            .await
            .map_err(api_error)?;
    } else {
        guard.check_for(command).await?;
        api.create(&params, &desired).await.map_err(api_error)?;
    }
    guard.check_for(command).await
}

pub async fn observe(
    client: Client,
    spec: &RuntimeSpec,
    observed: &mut ObservedState,
) -> Result<()> {
    if spec.autoscaling.is_none() || spec.replicas == 0 {
        return ensure_absent(client, spec).await;
    }
    let policy = spec.autoscaling.as_ref().unwrap();
    let Some(uid) = observed.runtime_id.as_deref() else {
        observed.ready = false;
        return Ok(());
    };
    let (namespace, name, _) = identity(spec)?;
    let api: Api<HorizontalPodAutoscaler> = Api::namespaced(client.clone(), &namespace);
    let Some(hpa) = api.get_opt(&name).await.map_err(api_error)? else {
        observed.ready = false;
        return Ok(());
    };
    verify(spec, uid, &json!(hpa), true)?;
    let deployments: Api<Deployment> = Api::namespaced(client, &namespace);
    let deployment = deployments.get(&name).await.map_err(api_error)?;
    if deployment.metadata.uid.as_deref() != Some(uid) {
        return Err(ExecutionError::Ownership);
    }
    let value = json!(hpa);
    let current = deployment
        .status
        .as_ref()
        .and_then(|d| d.replicas)
        .unwrap_or(0);
    let ready = deployment
        .status
        .as_ref()
        .and_then(|d| d.available_replicas)
        .unwrap_or(0);
    let desired = value["status"]["desiredReplicas"]
        .as_u64()
        .unwrap_or_else(|| {
            deployment
                .spec
                .as_ref()
                .and_then(|d| d.replicas)
                .unwrap_or(0)
                .max(0) as u64
        });
    if !(0..=20).contains(&current) || !(0..=20).contains(&ready) || desired > 20 {
        return Err(ExecutionError::Runtime);
    }
    let metric = value["status"]["conditions"]
        .as_array()
        .and_then(|conditions| conditions.iter().find(|c| c["type"] == "ScalingActive"));
    let metrics_available = metric.is_some_and(|c| c["status"] == "True");
    observed.autoscaling = Some(AutoscalingObservation {
        min_replicas: policy.min_replicas,
        max_replicas: policy.max_replicas,
        current_replicas: current as u16,
        ready_replicas: ready as u16,
        desired_replicas: desired as u16,
        metrics_available,
        reason: if metrics_available {
            "MetricsAvailable"
        } else {
            "MetricsUnavailable"
        }
        .into(),
    });
    observed.ready &=
        (i32::from(policy.min_replicas)..=i32::from(policy.max_replicas)).contains(&current);
    Ok(())
}

pub async fn ensure_absent(client: Client, spec: &RuntimeSpec) -> Result<()> {
    let (namespace, name, _) = identity(spec)?;
    let api: Api<HorizontalPodAutoscaler> = Api::namespaced(client, &namespace);
    if api.get_opt(&name).await.map_err(api_error)?.is_some() {
        return Err(ExecutionError::Ownership);
    }
    Ok(())
}
