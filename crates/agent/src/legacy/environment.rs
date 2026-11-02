use super::*;
use k8s_openapi::{
    ByteString,
    api::core::v1::{EnvVar, EnvVarSource, Secret, SecretKeySelector},
};
use kube::api::{DeleteParams, Preconditions};
fn secret_name(spec: &RuntimeSpec) -> String {
    format!("{}-env-{}", name(spec), spec.revision)
}
fn material(command: &Command) -> Result<Option<&EnvironmentMaterial>> {
    if command.spec.hosted_access.is_none() || command.action != Action::Apply {
        return Ok(None);
    }
    command.verify_material()?;
    Ok(Some(
        command
            .material
            .as_ref()
            .and_then(|m| m.environment.as_ref())
            .ok_or(ExecutionError::Denied)?,
    ))
}
pub(super) fn bind(deployment: &mut Deployment, command: &Command) -> Result<()> {
    let Some(environment) = material(command)? else {
        return Ok(());
    };
    let pod = deployment
        .spec
        .as_mut()
        .and_then(|s| s.template.spec.as_mut())
        .ok_or(ExecutionError::Ownership)?;
    if pod.containers.len() != 1 || pod.containers[0].name != "app" {
        return Err(ExecutionError::Ownership);
    }
    pod.containers[0].env_from = None;
    pod.containers[0].env = Some(
        environment
            .values
            .keys()
            .map(|key| EnvVar {
                name: key.clone(),
                value: None,
                value_from: Some(EnvVarSource {
                    secret_key_ref: Some(SecretKeySelector {
                        name: secret_name(&command.spec),
                        key: key.clone(),
                        optional: Some(false),
                    }),
                    ..Default::default()
                }),
            })
            .collect(),
    );
    Ok(())
}
pub(super) async fn prepare(client: Client, command: &Command, guard: &LeaseGuard) -> Result<()> {
    let Some(environment) = material(command)? else {
        return Ok(());
    };
    if environment.values.is_empty() {
        return Ok(());
    }
    let spec = &command.spec;
    let api: Api<Secret> = Api::namespaced(client, &namespace(spec));
    let data: BTreeMap<_, _> = environment
        .values
        .iter()
        .map(|(key, value)| (key.clone(), ByteString(value.as_bytes().to_vec())))
        .collect();
    if let Some(existing) = api.get_opt(&secret_name(spec)).await.map_err(api_failure)? {
        owner(&existing.metadata, spec, true)?;
        if existing.type_.as_deref() != Some("Opaque")
            || existing.immutable != Some(true)
            || existing.data.as_ref() != Some(&data)
            || existing
                .metadata
                .annotations
                .as_ref()
                .and_then(|a| a.get("webdock.dev/environment-source"))
                != Some(&environment.source_sha256)
        {
            return Err(ExecutionError::Ownership);
        }
        return Ok(());
    }
    let secret = Secret {
        metadata: ObjectMeta {
            name: Some(secret_name(spec)),
            namespace: Some(namespace(spec)),
            labels: Some(owner_labels(spec, true)?),
            annotations: Some(BTreeMap::from([
                ("webdock.dev/revision".into(), spec.revision.to_string()),
                (
                    "webdock.dev/environment-source".into(),
                    environment.source_sha256.clone(),
                ),
            ])),
            ..Default::default()
        },
        type_: Some("Opaque".into()),
        immutable: Some(true),
        data: Some(data),
        ..Default::default()
    };
    guard.check().await?;
    api.create(&PostParams::default(), &secret)
        .await
        .map_err(api_failure)?;
    Ok(())
}
pub(super) async fn cleanup(client: Client, spec: &RuntimeSpec, guard: &LeaseGuard) -> Result<()> {
    cleanup_kind(client, spec, guard, false).await
}
pub(super) async fn cleanup_kind(
    client: Client,
    spec: &RuntimeSpec,
    guard: &LeaseGuard,
    services: bool,
) -> Result<()> {
    if spec.hosted_access.is_none() {
        return Ok(());
    }
    let mut referenced = std::collections::BTreeSet::new();
    let mut keep = |pod: &k8s_openapi::api::core::v1::PodSpec| {
        for volume in pod.volumes.as_deref().unwrap_or_default() {
            if let Some(secret) = &volume.secret
                && let Some(name) = &secret.secret_name
            {
                referenced.insert(name.clone());
            }
        }
        for container in &pod.containers {
            for env in container.env.as_deref().unwrap_or_default() {
                if let Some(reference) = env
                    .value_from
                    .as_ref()
                    .and_then(|v| v.secret_key_ref.as_ref())
                {
                    referenced.insert(reference.name.clone());
                }
            }
            for env in container.env_from.as_deref().unwrap_or_default() {
                if let Some(reference) = &env.secret_ref {
                    referenced.insert(reference.name.clone());
                }
            }
        }
    };
    if let Some(deployment) = existing_deployment(client.clone(), spec).await?
        && let Some(pod) = deployment
            .spec
            .as_ref()
            .and_then(|d| d.template.spec.as_ref())
    {
        keep(pod);
    }
    let pods: Api<Pod> = Api::namespaced(client.clone(), &namespace(spec));
    let pods = pods
        .list(
            &ListParams::default()
                .labels(&format!("webdock.dev/app-id={}", spec.resource_id))
                .limit(201),
        )
        .await
        .map_err(api_failure)?;
    if pods.items.len() > 200 || pods.metadata.continue_.is_some_and(|s| !s.is_empty()) {
        return Err(ExecutionError::Runtime);
    }
    for pod in &pods.items {
        owner(&pod.metadata, spec, true)?;
        if let Some(pod) = &pod.spec {
            keep(pod);
        }
    }
    let api: Api<Secret> = Api::namespaced(client, &namespace(spec));
    let secrets = api
        .list(
            &ListParams::default()
                .labels(&format!("webdock.dev/app-id={}", spec.resource_id))
                .limit(201),
        )
        .await
        .map_err(api_failure)?;
    if secrets.items.len() > 200 || secrets.metadata.continue_.is_some_and(|s| !s.is_empty()) {
        return Err(ExecutionError::Runtime);
    }
    let prefix = format!(
        "{}-{}-",
        name(spec),
        if services { "services" } else { "env" }
    );
    for secret in secrets.items {
        let name = secret
            .metadata
            .name
            .as_deref()
            .ok_or(ExecutionError::Ownership)?;
        let Some(suffix) = name.strip_prefix(&prefix) else {
            continue;
        };
        let revision = suffix
            .parse::<u64>()
            .ok()
            .filter(|v| *v > 0 && *v <= spec.revision)
            .ok_or(ExecutionError::Ownership)?;
        if suffix != revision.to_string() {
            return Err(ExecutionError::Ownership);
        }
        if services {
            super::services::owner(&secret.metadata, spec)?;
        } else {
            owner(&secret.metadata, spec, true)?;
        }
        if referenced.contains(name) {
            continue;
        }
        if secret.type_.as_deref() != Some("Opaque") {
            return Err(ExecutionError::Ownership);
        }
        let params = DeleteParams {
            preconditions: Some(Preconditions {
                uid: Some(
                    secret
                        .metadata
                        .uid
                        .clone()
                        .ok_or(ExecutionError::Ownership)?,
                ),
                resource_version: Some(
                    secret
                        .metadata
                        .resource_version
                        .clone()
                        .ok_or(ExecutionError::Ownership)?,
                ),
            }),
            ..Default::default()
        };
        guard.check().await?;
        api.delete(name, &params).await.map_err(api_failure)?;
    }
    Ok(())
}
