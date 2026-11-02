//! Fixed installation-owned Kubernetes runtime profile. Core owns release/operation authority;
//! this executor owns only guarded Kubernetes application and nonsecret UID/convergence evidence.
mod profile;
use kube::{
    Api, Client,
    api::{
        ApiResource, DynamicObject, GroupVersionKind, ListParams, Patch, PatchParams, PostParams,
    },
};
pub use profile::{STAGE_SCRIPT, plugin_objects};
use serde_json::{Value, json};
use std::{future::Future, time::Duration};
use webdock_agent_protocol::plugin::*;
use webdock_supervisor::{ExecutionError, Result, timestamp};

#[async_trait::async_trait]
pub trait PluginRuntimeAuthority: Send + Sync {
    async fn guard(&self, _claim: &PluginRuntimeClaim) -> Result<()> {
        Err(ExecutionError::Denied)
    }
    async fn checkpoint(
        &self,
        _claim: &PluginRuntimeClaim,
        _checkpoint: PluginRuntimeCheckpoint,
    ) -> Result<()> {
        Err(ExecutionError::Denied)
    }
}
#[async_trait::async_trait]
pub trait PluginRuntimeExecutor: Send + Sync {
    async fn execute(
        &self,
        claim: &PluginRuntimeClaim,
        authority: &dyn PluginRuntimeAuthority,
    ) -> Result<PluginRuntimeCompletion>;
}
// Recovery preserves this owner; an API-default owner named "unknown" cannot be trusted.
const RUNTIME_FIELD_MANAGER: &str = "webdock-plugin-runtime";
fn runtime_create_params() -> PostParams {
    PostParams {
        field_manager: Some(RUNTIME_FIELD_MANAGER.into()),
        ..Default::default()
    }
}
fn runtime_patch_params() -> PatchParams {
    PatchParams {
        field_manager: Some(RUNTIME_FIELD_MANAGER.into()),
        ..Default::default()
    }
}

pub struct KubernetesPluginRuntime {
    client: Client,
    installation: String,
}
impl KubernetesPluginRuntime {
    pub fn new(client: Client, installation: String) -> Self {
        Self {
            client,
            installation,
        }
    }
    async fn guarded<T>(
        &self,
        claim: &PluginRuntimeClaim,
        authority: &dyn PluginRuntimeAuthority,
        work: impl Future<Output = Result<T>>,
    ) -> Result<T> {
        claim
            .validate(timestamp()?)
            .map_err(|_| ExecutionError::Expired)?;
        authority.guard(claim).await?;
        claim
            .validate(timestamp()?)
            .map_err(|_| ExecutionError::Expired)?;
        tokio::pin!(work);
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        interval.tick().await;
        loop {
            tokio::select! {
                result=&mut work=>{let result=result?;claim.validate(timestamp()?).map_err(|_|ExecutionError::Expired)?;authority.guard(claim).await?;return Ok(result);},
                _=interval.tick()=>{claim.validate(timestamp()?).map_err(|_|ExecutionError::Expired)?;authority.guard(claim).await?;},
            }
        }
    }
    fn api(&self, object: &Value) -> Result<Api<DynamicObject>> {
        let version = object["apiVersion"]
            .as_str()
            .ok_or(ExecutionError::Runtime)?;
        let kind = object["kind"].as_str().ok_or(ExecutionError::Runtime)?;
        let (group, version) = version.split_once('/').unwrap_or(("", version));
        let resource = ApiResource::from_gvk(&GroupVersionKind::gvk(group, version, kind));
        Ok(if kind == "Namespace" {
            Api::all_with(self.client.clone(), &resource)
        } else {
            Api::namespaced_with(
                self.client.clone(),
                object["metadata"]["namespace"]
                    .as_str()
                    .ok_or(ExecutionError::Runtime)?,
                &resource,
            )
        })
    }
    fn owned(
        actual: &DynamicObject,
        desired: &Value,
        spec: &PluginRuntimeSpec,
        uid: Option<&str>,
    ) -> Result<()> {
        let labels = actual
            .metadata
            .labels
            .as_ref()
            .ok_or(ExecutionError::Ownership)?;
        let fingerprint = spec.fingerprint().map_err(|_| ExecutionError::Ownership)?;
        if desired["metadata"]["labels"]
            .as_object()
            .is_none_or(|wanted| {
                wanted
                    .iter()
                    .any(|(k, v)| labels.get(k).map(String::as_str) != v.as_str())
            })
            || actual
                .metadata
                .annotations
                .as_ref()
                .and_then(|a| a.get("webdock.dev/spec-sha256"))
                != Some(&fingerprint)
            || actual.metadata.deletion_timestamp.is_some()
            || actual
                .metadata
                .owner_references
                .as_ref()
                .is_some_and(|r| !r.is_empty())
            || uid.is_some_and(|uid| actual.metadata.uid.as_deref() != Some(uid))
        {
            return Err(ExecutionError::Ownership);
        }
        match desired["kind"].as_str() {
            Some("Namespace") => {}
            Some("Secret") => {
                if actual.data["data"] != desired["data"]
                    || actual.data["type"] != "Opaque"
                    || actual.data["immutable"] != true
                {
                    return Err(ExecutionError::Ownership);
                }
            }
            _ => {
                let mut wanted = desired["spec"].clone();
                if desired["kind"] == "Deployment" {
                    wanted
                        .as_object_mut()
                        .ok_or(ExecutionError::Runtime)?
                        .remove("replicas");
                }
                let mut actual_spec = actual.data["spec"].clone();
                if desired["kind"] == "Deployment" {
                    actual_spec
                        .as_object_mut()
                        .ok_or(ExecutionError::Ownership)?
                        .remove("replicas");
                }
                if !profile_matches(&actual_spec, &wanted, None) {
                    return Err(ExecutionError::Ownership);
                }
            }
        }
        Ok(())
    }
    async fn object(
        &self,
        claim: &PluginRuntimeClaim,
        authority: &dyn PluginRuntimeAuthority,
        desired: &Value,
        expected_uid: Option<&str>,
    ) -> Result<DynamicObject> {
        let api = self.api(desired)?;
        let name = desired["metadata"]["name"]
            .as_str()
            .ok_or(ExecutionError::Runtime)?;
        let existing = self
            .guarded(claim, authority, async {
                api.get_opt(name).await.map_err(|_| ExecutionError::Runtime)
            })
            .await?;
        if claim.lease.action == PluginRuntimeAction::Inspect {
            let existing = existing.ok_or(ExecutionError::Ownership)?;
            Self::owned(&existing, desired, &claim.spec, expected_uid)?;
            return Ok(existing);
        }
        if let Some(existing) = existing {
            Self::owned(&existing, desired, &claim.spec, expected_uid)?;
            if desired["kind"] != "Deployment"
                || claim.lease.action != PluginRuntimeAction::Suspend
                    && existing.data["spec"]["replicas"] == desired["spec"]["replicas"]
            {
                return Ok(existing);
            }
            let mut patch = json!({"metadata":{"uid":existing.metadata.uid,"resourceVersion":existing.metadata.resource_version},"spec":{"replicas":desired["spec"]["replicas"]}});
            if claim.lease.action == PluginRuntimeAction::Suspend {
                // A fresh fence must advance resourceVersion even when replicas is already zero.
                patch["metadata"]["annotations"] = json!({"webdock.dev/suspension-fence":sha256(&serde_json::to_vec(&claim.lease).map_err(|_|ExecutionError::Runtime)?)});
            }
            return self
                .guarded(claim, authority, async {
                    api.patch(name, &runtime_patch_params(), &Patch::Merge(&patch))
                        .await
                        .map_err(|_| ExecutionError::Runtime)
                })
                .await;
        }
        if expected_uid.is_some() {
            return Err(ExecutionError::Ownership);
        }
        let object: DynamicObject =
            serde_json::from_value(desired.clone()).map_err(|_| ExecutionError::Runtime)?;
        self.guarded(claim, authority, async {
            api.create(&runtime_create_params(), &object)
                .await
                .map_err(|_| ExecutionError::Runtime)
        })
        .await
    }
    async fn verify_objects(
        &self,
        claim: &PluginRuntimeClaim,
        authority: &dyn PluginRuntimeAuthority,
        objects: &[Value],
        checkpoint: &PluginRuntimeCheckpoint,
    ) -> Result<()> {
        let mut expected = checkpoint.clone();
        for (index, desired) in objects.iter().enumerate() {
            let api = self.api(desired)?;
            let name = desired["metadata"]["name"]
                .as_str()
                .ok_or(ExecutionError::Runtime)?;
            let object = self
                .guarded(claim, authority, async {
                    api.get(name).await.map_err(|_| ExecutionError::Runtime)
                })
                .await?;
            Self::owned(
                &object,
                desired,
                &claim.spec,
                checkpoint_field(&mut expected, index)?.as_deref(),
            )?;
        }
        Ok(())
    }
    async fn observed(
        &self,
        claim: &PluginRuntimeClaim,
        authority: &dyn PluginRuntimeAuthority,
        desired: &Value,
        checkpoint: &PluginRuntimeCheckpoint,
    ) -> Result<bool> {
        let api = self.api(desired)?;
        let object = self
            .guarded(claim, authority, async {
                api.get(&claim.spec.service)
                    .await
                    .map_err(|_| ExecutionError::Runtime)
            })
            .await?;
        Self::owned(
            &object,
            desired,
            &claim.spec,
            checkpoint.deployment_uid.as_deref(),
        )?;
        let status = &object.data["status"];
        let observed = object.metadata.generation.is_some_and(|g| {
            status["observedGeneration"]
                .as_i64()
                .is_some_and(|v| v >= g)
        });
        if !observed {
            return Ok(false);
        }
        let replicas = object.data["spec"]["replicas"].as_u64().unwrap_or(1);
        if claim.lease.action == PluginRuntimeAction::Suspend
            || claim.lease.action == PluginRuntimeAction::Inspect && replicas == 0
        {
            if !(replicas == 0
                && [
                    "replicas",
                    "readyReplicas",
                    "availableReplicas",
                    "updatedReplicas",
                    "terminatingReplicas",
                ]
                .iter()
                .all(|f| status[*f].as_u64().unwrap_or(0) == 0))
            {
                return Ok(false);
            }
            let pods: Api<k8s_openapi::api::core::v1::Pod> =
                Api::namespaced(self.client.clone(), &claim.spec.namespace);
            return self
                .guarded(claim, authority, async {
                    // Any remaining Pod, including a terminating/unready one, prevents another writer.
                    let pods = pods
                        .list(&ListParams::default().limit(1))
                        .await
                        .map_err(|_| ExecutionError::Runtime)?;
                    Ok(pods.items.is_empty()
                        && pods
                            .metadata
                            .continue_
                            .as_ref()
                            .is_none_or(String::is_empty))
                })
                .await;
        }
        Ok(replicas == 1
            && ["readyReplicas", "availableReplicas", "updatedReplicas"]
                .iter()
                .all(|f| status[*f].as_u64() == Some(1)))
    }
}
#[async_trait::async_trait]
impl PluginRuntimeExecutor for KubernetesPluginRuntime {
    async fn execute(
        &self,
        claim: &PluginRuntimeClaim,
        authority: &dyn PluginRuntimeAuthority,
    ) -> Result<PluginRuntimeCompletion> {
        if claim.spec.platform_id != self.installation {
            return Err(ExecutionError::Ownership);
        }
        if claim.lease.action == PluginRuntimeAction::Suspend
            && claim
                .checkpoint
                .as_ref()
                .is_none_or(|checkpoint| !checkpoint.complete())
        {
            return Err(ExecutionError::Ownership);
        }
        let objects = plugin_objects(claim)?;
        let mut checkpoint = claim.checkpoint.clone().unwrap_or_default();
        if claim.lease.action == PluginRuntimeAction::Suspend {
            self.verify_objects(claim, authority, &objects, &checkpoint)
                .await?;
        }
        // Isolation must exist before any Pod can start; route exposure comes last.
        for index in [0, 4, 1, 3, 2, 5] {
            let desired = &objects[index];
            // Recheck namespace custody before each namespaced mutation, including after a wait.
            if index > 0 {
                self.object(
                    claim,
                    authority,
                    &objects[0],
                    checkpoint.namespace_uid.as_deref(),
                )
                .await?;
            }
            let field = checkpoint_field(&mut checkpoint, index)?;
            let object = self
                .object(claim, authority, desired, field.as_deref())
                .await?;
            *field = Some(object.metadata.uid.ok_or(ExecutionError::Ownership)?);
            if index > 0 {
                self.object(
                    claim,
                    authority,
                    &objects[0],
                    checkpoint.namespace_uid.as_deref(),
                )
                .await?;
            }
            authority.checkpoint(claim, checkpoint.clone()).await?;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
        loop {
            self.verify_objects(claim, authority, &objects, &checkpoint)
                .await?;
            if self
                .observed(claim, authority, &objects[2], &checkpoint)
                .await?
            {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(ExecutionError::Runtime);
            }
            self.guarded(claim, authority, async {
                tokio::time::sleep(Duration::from_millis(500)).await;
                Ok(())
            })
            .await?;
        }
        self.verify_objects(claim, authority, &objects, &checkpoint)
            .await?;
        authority.guard(claim).await?;
        claim
            .validate(timestamp()?)
            .map_err(|_| ExecutionError::Expired)?;
        Ok(PluginRuntimeCompletion {
            outcome: PluginRuntimeOutcome::Succeeded,
            checkpoint,
            converged: true,
        })
    }
}
fn checkpoint_field(
    checkpoint: &mut PluginRuntimeCheckpoint,
    index: usize,
) -> Result<&mut Option<String>> {
    match index {
        0 => Ok(&mut checkpoint.namespace_uid),
        1 => Ok(&mut checkpoint.secret_uid),
        2 => Ok(&mut checkpoint.deployment_uid),
        3 => Ok(&mut checkpoint.service_uid),
        4 => Ok(&mut checkpoint.network_policy_uid),
        5 => Ok(&mut checkpoint.route_uid),
        _ => Err(ExecutionError::Runtime),
    }
}
// Kubernetes defaults harmless transport/controller fields; injected pod privileges, containers,
// environment, mounts, routes and egress are not defaults and must fail closed on recovery.
fn profile_matches(actual: &Value, wanted: &Value, key: Option<&str>) -> bool {
    match (actual, wanted) {
        (Value::Object(a), Value::Object(w)) => {
            w.iter().all(|(k, v)| {
                a.get(k).map_or_else(
                    || v.as_array().is_some_and(Vec::is_empty),
                    |actual| profile_matches(actual, v, Some(k)),
                )
            }) && a
                .iter()
                .all(|(k, v)| w.contains_key(k) || safe_default(k, v))
        }
        (Value::Array(a), Value::Array(w)) => {
            a.len() == w.len() && a.iter().zip(w).all(|(a, w)| profile_matches(a, w, None))
        }
        (Value::String(_), Value::String(_))
            if matches!(
                key,
                Some("cpu" | "memory" | "ephemeral-storage" | "sizeLimit")
            ) =>
        {
            let scale = if key == Some("cpu") {
                webdock_plugin_protocol::MILLI
            } else {
                (1, 1)
            };
            let amount = webdock_plugin_protocol::quantity(actual, scale);
            amount.is_some() && amount == webdock_plugin_protocol::quantity(wanted, scale)
        }
        _ => actual == wanted,
    }
}
fn safe_default(key: &str, value: &Value) -> bool {
    match key {
        "creationTimestamp" => value.is_null(),
        "serviceAccount" => value == "default",
        "sessionAffinity" => value == "None",
        "internalTrafficPolicy" => value == "Cluster",
        "ipFamilyPolicy" => value == "SingleStack" || value == "PreferDualStack",
        "clusterIP" => value
            .as_str()
            .is_some_and(|v| v.parse::<std::net::IpAddr>().is_ok()),
        "clusterIPs" => value.as_array().is_some_and(|v| {
            !v.is_empty()
                && v.len() <= 2
                && v.iter().all(|v| {
                    v.as_str()
                        .is_some_and(|v| v.parse::<std::net::IpAddr>().is_ok())
                })
        }),
        "ipFamilies" => value.as_array().is_some_and(|v| {
            !v.is_empty() && v.len() <= 2 && v.iter().all(|v| v == "IPv4" || v == "IPv6")
        }),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    mod fixture {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/support/plugin_runtime.rs"
        ));
    }
    struct Authority {
        calls: AtomicUsize,
        expires: bool,
    }
    #[async_trait::async_trait]
    impl PluginRuntimeAuthority for Authority {
        async fn guard(&self, claim: &PluginRuntimeClaim) -> Result<()> {
            if self.expires {
                while timestamp()? < claim.lease.lease_until {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                return Ok(());
            }
            if self.calls.fetch_add(1, Ordering::SeqCst) > 0 {
                return Err(ExecutionError::Denied);
            }
            Ok(())
        }
    }
    fn runtime() -> KubernetesPluginRuntime {
        let config = kube::Config::new("http://127.0.0.1:1".parse().unwrap());
        KubernetesPluginRuntime::new(Client::try_from(config).unwrap(), "fixture".into())
    }
    #[test]
    fn created_and_scaled_runtime_has_an_explicit_stable_replica_owner() {
        assert_eq!(
            runtime_create_params().field_manager.as_deref(),
            Some("webdock-plugin-runtime")
        );
        assert_eq!(
            runtime_patch_params().field_manager.as_deref(),
            Some("webdock-plugin-runtime")
        );
        assert!(!runtime_patch_params().force);
    }
    #[tokio::test]
    async fn delayed_permit_cannot_start_mutation_after_expiration() {
        let mut claim = fixture::claim();
        claim.lease.lease_until = timestamp().unwrap() + 1;
        let started = Arc::new(AtomicBool::new(false));
        let work = started.clone();
        let result = runtime()
            .guarded(
                &claim,
                &Authority {
                    calls: AtomicUsize::new(0),
                    expires: true,
                },
                async move {
                    work.store(true, Ordering::SeqCst);
                    Ok(())
                },
            )
            .await;
        assert!(result.is_err());
        assert!(!started.load(Ordering::SeqCst));
    }
    #[tokio::test]
    async fn revoked_authority_cancels_pending_kubernetes_work() {
        let claim = fixture::claim();
        let finished = Arc::new(AtomicBool::new(false));
        let work = finished.clone();
        let result = runtime()
            .guarded(
                &claim,
                &Authority {
                    calls: AtomicUsize::new(0),
                    expires: false,
                },
                async move {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    work.store(true, Ordering::SeqCst);
                    Ok(())
                },
            )
            .await;
        assert!(result.is_err());
        assert!(!finished.load(Ordering::SeqCst));
    }
    #[test]
    fn kubernetes_omitted_empty_aliases_are_equivalent_without_allowing_injected_fields() {
        let claim = fixture::claim();
        let objects = plugin_objects(&claim).unwrap();
        let desired = &objects[2];
        let mut actual = desired.clone();
        actual["spec"]["template"]["spec"]
            .as_object_mut()
            .unwrap()
            .remove("hostAliases");
        let actual: DynamicObject = serde_json::from_value(actual).unwrap();
        KubernetesPluginRuntime::owned(&actual, desired, &claim.spec, None).unwrap();
    }
    #[test]
    fn deprecated_service_account_alias_is_only_accepted_for_the_fixed_default_account() {
        let claim = fixture::claim();
        let objects = plugin_objects(&claim).unwrap();
        let desired = &objects[2];
        let mut actual = desired.clone();
        actual["spec"]["template"]["spec"]["serviceAccount"] = "default".into();
        let object: DynamicObject = serde_json::from_value(actual.clone()).unwrap();
        KubernetesPluginRuntime::owned(&object, desired, &claim.spec, None).unwrap();
        actual["spec"]["template"]["spec"]["serviceAccount"] = "privileged".into();
        let object: DynamicObject = serde_json::from_value(actual).unwrap();
        assert!(KubernetesPluginRuntime::owned(&object, desired, &claim.spec, None).is_err());
    }
    #[test]
    fn matching_labels_and_annotation_do_not_adopt_changed_objects_or_uids() {
        let claim = fixture::claim();
        let objects = plugin_objects(&claim).unwrap();
        let desired = &objects[2];
        let mut object = desired.clone();
        object["metadata"]["uid"] = "owned".into();
        let actual: DynamicObject = serde_json::from_value(object.clone()).unwrap();
        KubernetesPluginRuntime::owned(&actual, desired, &claim.spec, Some("owned")).unwrap();
        assert!(
            KubernetesPluginRuntime::owned(&actual, desired, &claim.spec, Some("replaced"))
                .is_err()
        );
        object["spec"]["template"]["spec"]["containers"][0]["env"] =
            json!([{"name":"EXFILTRATE","value":"bad"}]);
        let actual: DynamicObject = serde_json::from_value(object).unwrap();
        assert!(KubernetesPluginRuntime::owned(&actual, desired, &claim.spec, None).is_err());
    }
}
