use super::*;
use k8s_openapi::api::{
    core::v1::{Secret, Volume, VolumeMount},
    networking::v1::NetworkPolicy,
};
use serde_json::json;
fn references(command: &Command) -> Result<Option<&[ServiceBindingReference]>> {
    if command.action != Action::Apply {
        return Ok(None);
    }
    let Some(references) = command
        .spec
        .hosted_access
        .as_ref()
        .and_then(|a| a.service_bindings.as_deref())
    else {
        return Ok(None);
    };
    command.verify_material()?;
    Ok(Some(references))
}
/// Exact retained service Secret/mount and NetworkPolicy layout.
pub fn objects(command: &Command) -> Result<Option<(Option<Secret>, NetworkPolicy)>> {
    let Some(references) = references(command)? else {
        return Ok(None);
    };
    let spec = &command.spec;
    let labels = owner_labels(spec, true)?;
    let metadata = |name: String| json!({"name":name,"namespace":namespace(spec),"labels":labels,"annotations":{"webdock.dev/revision":spec.revision.to_string()}});
    let mut data = BTreeMap::new();
    for material in command
        .material
        .as_ref()
        .and_then(|m| m.service_bindings.as_ref())
        .ok_or(ExecutionError::Denied)?
    {
        if let ServiceBindingMaterial::Postgres {
            id, ca_certificate, ..
        } = material
        {
            data.insert(
                format!("binding-{id}.crt"),
                k8s_openapi::ByteString(ca_certificate.as_bytes().to_vec()),
            );
        }
    }
    let secret = if data.is_empty() {
        None
    } else {
        Some(serde_json::from_value(json!({"apiVersion":"v1","kind":"Secret","metadata":metadata(format!("{}-services-{}",name(spec),spec.revision)),"type":"Opaque","immutable":true,"data":data})).map_err(|_|ExecutionError::Invalid("service Secret"))?)
    };
    let backends = references
        .iter()
        .filter_map(|r| {
            if let ServiceBindingReference::Postgres { backend_id, .. } = r {
                Some(backend_id)
            } else {
                None
            }
        })
        .collect::<std::collections::BTreeSet<_>>();
    let mut egress=backends.iter().map(|id|json!({"to":[{"namespaceSelector":{"matchLabels":{"kubernetes.io/metadata.name":format!("wd-db-{id}")}},"podSelector":{"matchLabels":{"cnpg.io/cluster":"postgres"}}}],"ports":[{"protocol":"TCP","port":5432}]})).collect::<Vec<_>>();
    for reference in references {
        if let ServiceBindingReference::Mail { egress_cidrs, .. } = reference {
            egress.push(json!({"to":egress_cidrs.iter().map(|cidr|json!({"ipBlock":{"cidr":cidr}})).collect::<Vec<_>>(),"ports":[{"protocol":"TCP","port":443}]}));
        }
    }
    let mut policy:NetworkPolicy=serde_json::from_value(json!({"apiVersion":"networking.k8s.io/v1","kind":"NetworkPolicy","metadata":metadata(format!("{}-services",name(spec))),"spec":{"podSelector":{"matchLabels":{"webdock.dev/app-id":spec.resource_id}},"policyTypes":["Egress"],"egress":egress}})).map_err(|_|ExecutionError::Invalid("service policy"))?;
    if let Some(spec) = &mut policy.spec
        && spec.egress.as_ref().is_some_and(Vec::is_empty)
    {
        spec.egress = None;
    }
    Ok(Some((secret, policy)))
}
pub(super) fn bind(deployment: &mut Deployment, command: &Command) -> Result<()> {
    let Some(references) = references(command)? else {
        return Ok(());
    };
    let spec = &command.spec;
    let pod = deployment
        .spec
        .as_mut()
        .and_then(|d| d.template.spec.as_mut())
        .ok_or(ExecutionError::Ownership)?;
    if pod.containers.len() != 1 || pod.containers[0].name != "app" {
        return Err(ExecutionError::Ownership);
    }
    let volumes = pod.volumes.get_or_insert_with(Vec::new);
    volumes.retain(|v| v.name != "services");
    let mounts = pod.containers[0].volume_mounts.get_or_insert_with(Vec::new);
    mounts.retain(|v| v.name != "services");
    let items = references
        .iter()
        .filter_map(|r| {
            if let ServiceBindingReference::Postgres { id, .. } = r {
                Some(json!({"key":format!("binding-{id}.crt"),"path":format!("{id}/ca.crt")}))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    if !items.is_empty() {
        let volume:Volume=serde_json::from_value(json!({"name":"services","secret":{"secretName":format!("{}-services-{}",name(spec),spec.revision),"items":items}})).map_err(|_|ExecutionError::Invalid("service volume"))?;
        volumes.push(volume);
        mounts.push(VolumeMount {
            name: "services".into(),
            mount_path: "/var/run/webdock/services".into(),
            read_only: Some(true),
            ..Default::default()
        });
    }
    Ok(())
}
pub(super) async fn prepare(client: Client, command: &Command, guard: &LeaseGuard) -> Result<()> {
    let Some((secret, mut policy)) = objects(command)? else {
        return Ok(());
    };
    let spec = &command.spec;
    if let Some(secret) = secret {
        let api: Api<Secret> = Api::namespaced(client.clone(), &namespace(spec));
        let name = secret
            .metadata
            .name
            .as_deref()
            .ok_or(ExecutionError::Ownership)?;
        if let Some(existing) = api.get_opt(name).await.map_err(api_failure)? {
            owner(&existing.metadata, spec)?;
            if existing.immutable != Some(true)
                || existing.type_.as_deref() != Some("Opaque")
                || existing.data != secret.data
            {
                return Err(ExecutionError::Ownership);
            }
        } else {
            guard.check().await?;
            api.create(&PostParams::default(), &secret)
                .await
                .map_err(api_failure)?;
        }
        let actual = api.get(name).await.map_err(api_failure)?;
        owner(&actual.metadata, spec)?;
        if actual.immutable != Some(true)
            || actual.type_.as_deref() != Some("Opaque")
            || actual.data != secret.data
        {
            return Err(ExecutionError::Ownership);
        }
    }
    let api: Api<NetworkPolicy> = Api::namespaced(client, &namespace(spec));
    let name = policy
        .metadata
        .name
        .clone()
        .ok_or(ExecutionError::Ownership)?;
    if let Some(existing) = api.get_opt(&name).await.map_err(api_failure)? {
        owner(&existing.metadata, spec)?;
        policy.metadata.uid = existing.metadata.uid;
        policy.metadata.resource_version = existing.metadata.resource_version;
        guard.check().await?;
        api.replace(&name, &PostParams::default(), &policy)
            .await
            .map_err(api_failure)?;
    } else {
        guard.check().await?;
        api.create(&PostParams::default(), &policy)
            .await
            .map_err(api_failure)?;
    }
    let actual = api.get(&name).await.map_err(api_failure)?;
    owner(&actual.metadata, spec)?;
    if actual.spec != policy.spec {
        return Err(ExecutionError::Ownership);
    }
    Ok(())
}
pub(super) async fn cleanup(client: Client, spec: &RuntimeSpec, guard: &LeaseGuard) -> Result<()> {
    if spec
        .hosted_access
        .as_ref()
        .is_some_and(|a| a.service_bindings.is_some())
    {
        environment::cleanup_kind(client, spec, guard, true).await?;
    }
    Ok(())
}

// Retained service resources have exact owner labels and versioned names but historically no revision annotation.
pub(super) fn owner(metadata: &ObjectMeta, spec: &RuntimeSpec) -> Result<()> {
    let labels = metadata.labels.as_ref().ok_or(ExecutionError::Ownership)?;
    if owner_labels(spec, true)?
        .iter()
        .any(|(k, v)| labels.get(k) != Some(v))
    {
        return Err(ExecutionError::Ownership);
    }
    if let Some(revision) = metadata
        .annotations
        .as_ref()
        .and_then(|a| a.get("webdock.dev/revision"))
    {
        let revision = revision
            .parse::<u64>()
            .map_err(|_| ExecutionError::Ownership)?;
        if revision == 0 || revision > spec.revision {
            return Err(ExecutionError::Fenced);
        }
    }
    Ok(())
}
