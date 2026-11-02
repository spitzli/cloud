use super::*;
use kube::{
    Api,
    api::{
        ApiResource, DeleteParams, DynamicObject, GroupVersionKind, ListParams, Patch, PatchParams,
        PostParams, Preconditions,
    },
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Mutex, time::Duration};
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Namespace,
    Deployment,
    Service,
    Volume,
    Claim,
    Secret,
    Quota,
    Limits,
    Account,
    Policy,
    Pod,
    ReplicaSet,
}
impl Kind {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Namespace => "Namespace",
            Self::Deployment => "Deployment",
            Self::Service => "Service",
            Self::Volume => "PersistentVolume",
            Self::Claim => "PersistentVolumeClaim",
            Self::Secret => "Secret",
            Self::Quota => "ResourceQuota",
            Self::Limits => "LimitRange",
            Self::Account => "ServiceAccount",
            Self::Policy => "NetworkPolicy",
            Self::Pod => "Pod",
            Self::ReplicaSet => "ReplicaSet",
        }
    }
    fn resource(self) -> ApiResource {
        let (group, version) = match self {
            Self::Deployment | Self::ReplicaSet => ("apps", "v1"),
            Self::Policy => ("networking.k8s.io", "v1"),
            _ => ("", "v1"),
        };
        ApiResource::from_gvk(&GroupVersionKind::gvk(group, version, self.name()))
    }
    fn namespaced(self) -> bool {
        !matches!(self, Self::Namespace | Self::Volume)
    }
}
pub(super) struct Execution<'a> {
    pub runtime: &'a KubernetesMailRuntime,
    pub claim: &'a MailClaim,
    pub authority: &'a dyn MailAuthority,
    ids: Mutex<BTreeMap<String, String>>,
}
impl<'a> Execution<'a> {
    pub(super) fn new(
        runtime: &'a KubernetesMailRuntime,
        claim: &'a MailClaim,
        authority: &'a dyn MailAuthority,
    ) -> Self {
        Self {
            runtime,
            claim,
            authority,
            ids: Mutex::new(BTreeMap::new()),
        }
    }
    pub(super) fn namespace(&self) -> String {
        format!("wd-mail-{}", self.claim.spec.customer_id)
    }
    pub(super) fn name(&self) -> String {
        format!("app-{}", self.claim.spec.customer_id)
    }
    pub(super) fn api(&self, kind: Kind) -> Api<DynamicObject> {
        let resource = kind.resource();
        if kind.namespaced() {
            Api::namespaced_with(self.runtime.client.clone(), &self.namespace(), &resource)
        } else {
            Api::all_with(self.runtime.client.clone(), &resource)
        }
    }
    pub(super) async fn guard(&self) -> Result<()> {
        self.claim
            .validate(chrono::Utc::now())
            .map_err(|_| ExecutionError::Expired)?;
        self.authority.guard(self.claim).await?;
        self.runtime.verify_configuration()?;
        if self.claim.spec.image != self.runtime.config.image
            || self.claim.spec.cluster_id != self.runtime.storage.1.cluster_id.as_str()
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(())
    }
    pub(super) async fn guarded<T>(
        &self,
        work: impl std::future::Future<Output = Result<T>>,
    ) -> Result<T> {
        self.guard().await?;
        tokio::pin!(work);
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        interval.tick().await;
        loop {
            tokio::select! {value=&mut work=>{let value=value?;self.guard().await?;return Ok(value)},_=interval.tick()=>self.guard().await?,}
        }
    }
    fn binding_uid(&self, kind: Kind, name: &str) -> Option<&str> {
        let binding = self
            .runtime
            .config
            .retained
            .get(&self.claim.spec.customer_id)?;
        match kind {
            Kind::Namespace => Some(&binding.namespace_uid),
            Kind::Deployment => Some(&binding.deployment_uid),
            Kind::Service => Some(&binding.service_uid),
            Kind::Volume => Some(&binding.volume_uid),
            Kind::Claim => Some(&binding.claim_uid),
            Kind::Secret => binding
                .bootstrap_secret
                .as_ref()
                .filter(|s| s.name == name)
                .map(|s| s.uid.as_str()),
            Kind::Policy => binding.policies.get(name).map(|p| p.uid.as_str()),
            _ => None,
        }
    }
    pub(super) fn capture(&self, kind: Kind, object: &DynamicObject) -> Result<()> {
        let name = object
            .metadata
            .name
            .as_ref()
            .ok_or(ExecutionError::Ownership)?;
        let uid = object
            .metadata
            .uid
            .as_ref()
            .ok_or(ExecutionError::Ownership)?;
        if uuid::Uuid::parse_str(uid).is_err()
            || object
                .metadata
                .resource_version
                .as_ref()
                .is_none_or(|v| v.is_empty())
            || (kind != Kind::Pod && object.metadata.deletion_timestamp.is_some())
            || object.metadata.namespace.as_deref()
                != kind.namespaced().then(|| self.namespace()).as_deref()
        {
            return Err(ExecutionError::Ownership);
        }
        if self
            .binding_uid(kind, name)
            .is_some_and(|expected| expected != uid)
        {
            return Err(ExecutionError::Ownership);
        }
        if kind == Kind::Deployment
            && self
                .claim
                .checkpoint
                .as_ref()
                .is_some_and(|c| c.runtime_id != *uid)
        {
            return Err(ExecutionError::Ownership);
        }
        let mut ids = self.ids.lock().map_err(|_| ExecutionError::Runtime)?;
        let key = format!("{}:{name}", kind.name());
        if ids.get(&key).is_some_and(|expected| expected != uid) {
            return Err(ExecutionError::Ownership);
        }
        ids.insert(key, uid.clone());
        Ok(())
    }
    pub(super) fn owner(&self, kind: Kind, object: &DynamicObject) -> Result<()> {
        self.capture(kind, object)?;
        let name = object
            .metadata
            .name
            .as_deref()
            .ok_or(ExecutionError::Ownership)?;
        let labels = object.metadata.labels.clone().unwrap_or_default();
        let expected = manifests::labels(
            self.claim,
            &self.runtime.config,
            matches!(
                kind,
                Kind::Deployment | Kind::Service | Kind::Volume | Kind::Claim | Kind::Secret
            ),
        )?;
        if kind == Kind::Account
            && name == "default"
            && labels.is_empty()
            && object
                .metadata
                .owner_references
                .as_ref()
                .is_none_or(|v| v.is_empty())
        {
            return Ok(());
        }
        for key in [
            "security.webdock.dev/managed-by",
            "webdock.dev/project-id",
            "webdock.dev/cluster-id",
        ] {
            if labels.get(key) != expected.get(key) {
                return Err(ExecutionError::Ownership);
            }
        }
        if expected.contains_key("webdock.dev/app-id")
            && labels.get("webdock.dev/app-id") != expected.get("webdock.dev/app-id")
        {
            return Err(ExecutionError::Ownership);
        }
        let native = labels.get("webdock.dev/mail-installation");
        if native.is_some() {
            if native != expected.get("webdock.dev/mail-installation")
                || object
                    .metadata
                    .annotations
                    .as_ref()
                    .and_then(|a| a.get("webdock.dev/mail-profile"))
                    != Some(&manifests::profile(self.claim)?)
            {
                return Err(ExecutionError::Ownership);
            }
        } else {
            let retained = self
                .runtime
                .config
                .retained
                .get(&self.claim.spec.customer_id)
                .ok_or(ExecutionError::Ownership)?;
            let namespace_key = format!("Namespace:{}", self.namespace());
            if kind != Kind::Namespace
                && self
                    .ids
                    .lock()
                    .map_err(|_| ExecutionError::Runtime)?
                    .get(&namespace_key)
                    != Some(&retained.namespace_uid)
            {
                return Err(ExecutionError::Ownership);
            }
            if !matches!(
                kind,
                Kind::Quota | Kind::Limits | Kind::Account | Kind::Policy
            ) && self.binding_uid(kind, name).is_none()
            {
                return Err(ExecutionError::Ownership);
            }
        }
        if let Some(annotations) = &object.metadata.annotations {
            for (key, maximum) in [
                ("webdock.dev/revision", self.claim.spec.revision),
                (
                    "webdock.dev/mail-generation",
                    i64::from(self.claim.spec.agent_generation),
                ),
            ] {
                if let Some(value) = annotations.get(key) {
                    let parsed = value
                        .parse::<i64>()
                        .map_err(|_| ExecutionError::Ownership)?;
                    if parsed < 1 || parsed > maximum || parsed.to_string() != *value {
                        return Err(ExecutionError::Fenced);
                    }
                }
            }
        }
        Ok(())
    }
    pub(super) async fn get(&self, kind: Kind, name: &str) -> Result<Option<DynamicObject>> {
        let object = self
            .guarded(async {
                self.api(kind)
                    .get_opt(name)
                    .await
                    .map_err(|_| ExecutionError::Runtime)
            })
            .await?;
        if let Some(object) = &object {
            self.owner(kind, object).inspect_err(|_|{tracing::warn!(resource_kind=kind.name(),resource_name=%name,phase="read-owner",observed_uid=?object.metadata.uid,expected_uid=?self.binding_uid(kind,name),observed_namespace=?object.metadata.namespace,"Mail ownership verification failed");})?;
        } else if self
            .ids
            .lock()
            .map_err(|_| ExecutionError::Runtime)?
            .contains_key(&format!("{}:{name}", kind.name()))
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(object)
    }
    pub(super) async fn list(
        &self,
        kind: Kind,
        selector: Option<&str>,
    ) -> Result<Vec<DynamicObject>> {
        let params = match selector {
            Some(selector) => ListParams::default().labels(selector).limit(65),
            None => ListParams::default().limit(65),
        };
        let list = self
            .guarded(async {
                self.api(kind)
                    .list(&params)
                    .await
                    .map_err(|_| ExecutionError::Runtime)
            })
            .await?;
        if list.items.len() > 64 || list.metadata.continue_.is_some_and(|s| !s.is_empty()) {
            return Err(ExecutionError::Ownership);
        }
        Ok(list.items)
    }
    pub(super) async fn apply(
        &self,
        kind: Kind,
        desired: &impl Serialize,
    ) -> Result<DynamicObject> {
        let desired =
            serde_json::to_value(desired).map_err(|_| ExecutionError::Invalid("Mail manifest"))?;
        let name = desired["metadata"]["name"]
            .as_str()
            .ok_or(ExecutionError::Invalid("Mail resource name"))?
            .to_owned();
        for _ in 0..3 {
            let current = self.get(kind, &name).await?;
            let mut value = desired.clone();
            if let Some(current) = &current {
                self.validate_existing(kind,current,&value).inspect_err(|_|{tracing::warn!(resource_kind=kind.name(),resource_name=%name,phase="existing-profile",mismatch_path=?mismatch_path(&current.data["spec"],&value["spec"],"spec"),"Mail existing profile differs");})?;
                if kind == Kind::Deployment {
                    preserve_template_annotations(&mut value, &current.data);
                }
                if kind == Kind::Volume
                    && current.data["spec"]
                        .get("storageClassName")
                        .is_none_or(Value::is_null)
                {
                    value["spec"]
                        .as_object_mut()
                        .ok_or(ExecutionError::Ownership)?
                        .remove("storageClassName");
                }
                let observed =
                    serde_json::to_value(current).map_err(|_| ExecutionError::Runtime)?;
                let handoff = creation_handoff(current,&value,self.claim).inspect_err(|_|{tracing::warn!(resource_kind=kind.name(),resource_name=%name,phase="creation-custody","Mail creation ownership differs");})?;
                if contains(&observed, &value) {
                    return Ok(current.clone());
                }
                value["metadata"]["uid"] = json!(current.metadata.uid);
                value["metadata"]["resourceVersion"] = json!(current.metadata.resource_version);
                self.guard().await?;
                let params = if handoff {
                    PatchParams::apply("webdock").force()
                } else {
                    PatchParams::apply("webdock")
                };
                match self
                    .api(kind)
                    .patch(&name, &params, &Patch::Apply(&value))
                    .await
                {
                    Ok(updated) => {
                        self.guard().await?;
                        self.owner(kind, &updated)?;
                        return Ok(updated);
                    }
                    Err(kube::Error::Api(error)) if error.code == 409 => {
                        tracing::warn!(resource_kind=kind.name(),resource_name=%name,field_manager_conflict=error.message.starts_with("Apply failed"),"Mail conditional apply rejected");
                        continue;
                    }
                    Err(_) => return Err(ExecutionError::Runtime),
                }
            }
            value["metadata"]["annotations"]["webdock.dev/mail-creation-operation"] =
                json!(self.claim.spec.operation_id);
            value["metadata"]["annotations"]["webdock.dev/mail-creation-generation"] =
                json!(self.claim.spec.agent_generation.to_string());
            value["metadata"]["annotations"]["webdock.dev/mail-creation-revision"] =
                json!(self.claim.spec.revision.to_string());
            let object: DynamicObject = serde_json::from_value(value)
                .map_err(|_| ExecutionError::Invalid("Mail resource"))?;
            let created = self
                .guarded(async {
                    self.api(kind)
                        .create(
                            &PostParams {
                                field_manager: Some("webdock-mail-create".into()),
                                ..Default::default()
                            },
                            &object,
                        )
                        .await
                        .map_err(|_| ExecutionError::Runtime)
                })
                .await?;
            self.owner(kind, &created)?;
            // Keep creation custody; Apply shares unchanged fields and takes changed fields later.
        }
        Err(ExecutionError::Conflict)
    }
    pub(super) fn validate_existing(
        &self,
        kind: Kind,
        current: &DynamicObject,
        desired: &Value,
    ) -> Result<()> {
        match kind {
            Kind::Secret => {
                if current.data.get("data") != desired.get("data")
                    || current.data.get("immutable") != Some(&json!(true))
                    || current.data.get("type") != Some(&json!("Opaque"))
                {
                    return Err(ExecutionError::Ownership);
                }
            }
            Kind::Volume | Kind::Claim => {
                let spec = current.data.get("spec").ok_or(ExecutionError::Ownership)?;
                if !storage_spec_matches(kind, spec, &desired["spec"]) {
                    return Err(ExecutionError::Ownership);
                }
            }
            Kind::Service => {
                let spec = &current.data["spec"];
                if spec["externalIPs"]
                    .as_array()
                    .is_some_and(|v| !v.is_empty())
                    || spec["type"] != "ClusterIP"
                    || spec["selector"] != desired["spec"]["selector"]
                    || !contains(&spec["ports"], &desired["spec"]["ports"])
                    || spec["ports"].as_array().is_none_or(|ports| {
                        ports.iter().any(|port| {
                            port["protocol"] != "TCP"
                                || port
                                    .get("nodePort")
                                    .is_some_and(|value| !value.is_null() && value != 0)
                        })
                    })
                {
                    return Err(ExecutionError::Ownership);
                }
            }
            Kind::Deployment => {
                super::lifecycle::validate_workload(current, self.claim, &self.runtime.config)?
            }
            _ => {}
        }
        Ok(())
    }
    pub(super) async fn remove_recovery(&self) -> Result<()> {
        for _ in 0..3 {
            let current = self
                .get(Kind::Deployment, &self.name())
                .await?
                .ok_or(ExecutionError::Ownership)?;
            super::lifecycle::validate_workload(&current, self.claim, &self.runtime.config)?;
            if !super::lifecycle::recovery_env(&current.data["spec"]["template"]["spec"]) {
                return Ok(());
            }
            let patch = recovery_removal(&current)?;
            // Omitting these creation-owned entries from Apply does not remove them.
            // Only this fixed list deletion may cross the recovery boundary.
            reject_foreign_fields(&current, &patch)?;
            self.guard().await?;
            match self
                .api(Kind::Deployment)
                .patch(
                    &self.name(),
                    &PatchParams {
                        field_manager: Some("webdock-mail-recovery".into()),
                        ..Default::default()
                    },
                    &Patch::Strategic(&patch),
                )
                .await
            {
                Ok(updated) => {
                    self.guard().await?;
                    self.owner(Kind::Deployment, &updated)?;
                    super::lifecycle::validate_workload(
                        &updated,
                        self.claim,
                        &self.runtime.config,
                    )?;
                    if super::lifecycle::recovery_env(&updated.data["spec"]["template"]["spec"]) {
                        return Err(ExecutionError::Ownership);
                    }
                    return Ok(());
                }
                Err(kube::Error::Api(error)) if error.code == 409 => {}
                Err(_) => return Err(ExecutionError::Runtime),
            }
        }
        Err(ExecutionError::Conflict)
    }
    pub(super) async fn delete(&self, kind: Kind, object: &DynamicObject) -> Result<()> {
        if matches!(kind, Kind::Pod) {
            self.capture(kind, object)?;
        } else {
            self.owner(kind, object)?;
        }
        let name = object
            .metadata
            .name
            .as_deref()
            .ok_or(ExecutionError::Ownership)?;
        let parameters = DeleteParams {
            preconditions: Some(Preconditions {
                uid: object.metadata.uid.clone(),
                resource_version: object.metadata.resource_version.clone(),
            }),
            ..Default::default()
        };
        self.guarded(async {
            self.api(kind)
                .delete(name, &parameters)
                .await
                .map_err(|_| ExecutionError::Runtime)
        })
        .await?;
        Ok(())
    }
}
pub(super) fn contains(actual: &Value, desired: &Value) -> bool {
    match desired {
        Value::Object(wanted) => actual.as_object().is_some_and(|actual| {
            wanted.iter().all(|(key, value)| {
                value.is_null() || contains(actual.get(key).unwrap_or(&Value::Null), value)
            })
        }),
        Value::Array(wanted) => actual.as_array().is_some_and(|actual| {
            actual.len() == wanted.len() && actual.iter().zip(wanted).all(|(a, d)| contains(a, d))
        }),
        _ => actual == desired,
    }
}
fn preserve_template_annotations(desired: &mut Value, current: &Value) {
    let existing = &current["spec"]["template"]["metadata"]["annotations"];
    let same_phase = super::lifecycle::recovery_env(&current["spec"]["template"]["spec"])
        == super::lifecycle::recovery_env(&desired["spec"]["template"]["spec"]);
    for key in ["webdock.dev/operation", "webdock.dev/revision"] {
        if let Some(value) = existing.get(key) {
            desired["spec"]["template"]["metadata"]["annotations"][key] = value.clone();
        }
    }
    if same_phase && !super::lifecycle::recovery_env(&desired["spec"]["template"]["spec"]) {
        if let Some(value) = existing.get("webdock.dev/mail-phase") {
            desired["spec"]["template"]["metadata"]["annotations"]["webdock.dev/mail-phase"] =
                value.clone();
        } else if let Some(annotations) =
            desired["spec"]["template"]["metadata"]["annotations"].as_object_mut()
        {
            annotations.remove("webdock.dev/mail-phase");
        }
    }
}

#[async_trait::async_trait]
impl MailAuthority for Execution<'_> {
    async fn guard(&self, claim: &MailClaim) -> Result<()> {
        if claim
            .spec
            .fingerprint()
            .map_err(|_| ExecutionError::Denied)?
            != self
                .claim
                .spec
                .fingerprint()
                .map_err(|_| ExecutionError::Denied)?
            || claim.lease.lease_token != self.claim.lease.lease_token
        {
            return Err(ExecutionError::Denied);
        }
        Execution::guard(self).await
    }
    async fn checkpoint(
        &self,
        claim: &MailClaim,
        checkpoint: webdock_agent_protocol::mail::MailCheckpoint,
    ) -> Result<()> {
        MailAuthority::guard(self, claim).await?;
        self.authority.checkpoint(claim, checkpoint).await
    }
}

fn storage_spec_matches(kind: Kind, current: &Value, desired: &Value) -> bool {
    let mut current = current.clone();
    // PV's empty storage class is omitted by the API; PVC's explicit empty string is distinct.
    if kind == Kind::Volume && current.get("storageClassName").is_none_or(Value::is_null) {
        current["storageClassName"] = json!("");
    }
    contains(&current, desired)
}

pub(super) fn mismatch_path(actual: &Value, desired: &Value, path: &str) -> Option<String> {
    if contains(actual, desired) {
        return None;
    }
    match desired {
        Value::Object(fields) => fields.iter().find_map(|(key, value)| {
            if value.is_null() {
                None
            } else {
                mismatch_path(
                    actual.get(key).unwrap_or(&Value::Null),
                    value,
                    &format!("{path}.{key}"),
                )
            }
        }),
        Value::Array(fields) => fields
            .iter()
            .enumerate()
            .find_map(|(index, value)| {
                mismatch_path(
                    actual.get(index).unwrap_or(&Value::Null),
                    value,
                    &format!("{path}[{index}]"),
                )
            })
            .or_else(|| Some(path.into())),
        _ => Some(path.into()),
    }
}

fn recovery_removal(current: &DynamicObject) -> Result<Value> {
    let uid = current
        .metadata
        .uid
        .as_ref()
        .ok_or(ExecutionError::Ownership)?;
    let version = current
        .metadata
        .resource_version
        .as_ref()
        .ok_or(ExecutionError::Ownership)?;
    Ok(
        json!({"metadata":{"uid":uid,"resourceVersion":version},"spec":{"template":{"spec":{"containers":[{"name":"app","env":[{"name":"STALWART_RECOVERY_MODE","$patch":"delete"},{"name":"STALWART_RECOVERY_ADMIN","$patch":"delete"}]}]}}}}),
    )
}
fn creation_handoff(current: &DynamicObject, desired: &Value, claim: &MailClaim) -> Result<bool> {
    let fields = current
        .metadata
        .managed_fields
        .as_deref()
        .unwrap_or_default();
    if !fields.iter().any(|entry| {
        entry.manager.as_deref() == Some("webdock-mail-create")
            && entry.operation.as_deref() == Some("Update")
            && entry.subresource.as_deref().is_none_or(str::is_empty)
    }) {
        return Ok(false);
    }
    if current
        .metadata
        .labels
        .as_ref()
        .is_none_or(|labels| !labels.contains_key("webdock.dev/mail-installation"))
        || current
            .metadata
            .annotations
            .as_ref()
            .is_none_or(|annotations| {
                !annotations.contains_key("webdock.dev/mail-creation-operation")
            })
    {
        // Reviewed retained resources use ordinary Apply; they never inherit native force custody.
        return Ok(false);
    }
    let annotations = current
        .metadata
        .annotations
        .as_ref()
        .ok_or(ExecutionError::Ownership)?;
    if annotations
        .get("webdock.dev/mail-creation-operation")
        .is_none_or(|operation| webdock_core::id::RecordId::parse(operation).is_err())
    {
        return Err(ExecutionError::Ownership);
    }
    for (key, maximum) in [
        (
            "webdock.dev/mail-creation-generation",
            i64::from(claim.spec.agent_generation),
        ),
        ("webdock.dev/mail-creation-revision", claim.spec.revision),
    ] {
        let raw = annotations.get(key).ok_or(ExecutionError::Ownership)?;
        let value = raw.parse::<i64>().map_err(|_| ExecutionError::Ownership)?;
        if value < 1 || value > maximum || value.to_string() != *raw {
            return Err(ExecutionError::Ownership);
        }
    }
    reject_foreign_fields(current, desired)?;
    Ok(true)
}
fn reject_foreign_fields(current: &DynamicObject, desired: &Value) -> Result<()> {
    for entry in current
        .metadata
        .managed_fields
        .as_deref()
        .unwrap_or_default()
    {
        if (entry.manager.as_deref() == Some("webdock-mail-create")
            && entry.operation.as_deref() == Some("Update"))
            || (entry.manager.as_deref() == Some("webdock")
                && entry.operation.as_deref() == Some("Apply"))
            || entry.subresource.as_deref().is_some_and(|s| !s.is_empty())
        {
            continue;
        }
        if entry
            .fields_v1
            .as_ref()
            .is_none_or(|fields| fields_overlap(&fields.0, desired))
        {
            tracing::warn!(manager=?entry.manager,resource_name=?current.metadata.name,"Mail refuses overlapping foreign field ownership");
            return Err(ExecutionError::Ownership);
        }
    }
    Ok(())
}
fn fields_overlap(fields: &Value, desired: &Value) -> bool {
    let Some(fields) = fields.as_object() else {
        return true;
    };
    if fields.is_empty() {
        return !desired.is_null();
    }
    fields.iter().any(|(key, children)| {
        if key == "." {
            return fields.len() == 1 && !desired.is_null();
        }
        if let Some(name) = key.strip_prefix("f:") {
            return desired
                .get(name)
                .is_some_and(|value| !value.is_null() && fields_overlap(children, value));
        }
        // Structured list selectors are conservative: never force a foreign-owned list.
        !desired.is_null()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use webdock_agent_protocol::mail::*;
    struct Authority;
    #[async_trait::async_trait]
    impl MailAuthority for Authority {}
    fn claim() -> MailClaim {
        let material = MailMaterial::Credentials {
            bootstrap_password: None,
            username: "webdock-admin@mail-123.example.test".into(),
            password: "fixture-only".into(),
        };
        MailClaim{spec:serde_json::from_value(json!({"version":1,"operationID":"456","customerID":"123","clusterID":"789","revision":1,"agentGeneration":1,"instanceKey":"mail-123","hostname":"mail-123.example.test","image":format!("registry.example.test/mail@sha256:{}","a".repeat(64)),"resources":MailResources::FIXED,"materialSha256":material.fingerprint().unwrap(),"action":"ensure"})).unwrap(),material,lease:MailLease{lease_token:uuid::Uuid::new_v4().to_string()},lease_until:chrono::Utc::now()+chrono::Duration::seconds(120),checkpoint:None}
    }
    fn runtime(claim: &MailClaim) -> KubernetesMailRuntime {
        let config=serde_json::from_value(json!({"installation":"fixture","node":"fixture-node","image":claim.spec.image,"retained":{}})).unwrap();
        let storage=serde_json::from_value(json!({"root":"/private/fixture","node":"fixture-node","clusterID":"789","poolID":"123e4567-e89b-42d3-a456-426614174000","capacityBytes":10737418240_u64,"reserveBytes":67108864})).unwrap();
        KubernetesMailRuntime {
            client: kube::Client::try_from(kube::Config::new(
                "https://127.0.0.1:1".parse().unwrap(),
            ))
            .unwrap(),
            config,
            config_path: "/private/unread".into(),
            storage: (StorageClient::new("/private/unreachable".into()), storage),
        }
    }
    fn object(execution: &Execution<'_>, kind: Kind, uid: &str, native: bool) -> DynamicObject {
        let mut labels = manifests::labels(
            execution.claim,
            &execution.runtime.config,
            kind == Kind::Deployment,
        )
        .unwrap();
        if !native {
            labels.remove("webdock.dev/mail-installation");
        }
        serde_json::from_value(json!({"apiVersion":kind.resource().api_version,"kind":kind.name(),"metadata":{"name":if kind==Kind::Namespace{execution.namespace()}else{execution.name()},"namespace":kind.namespaced().then(||execution.namespace()),"uid":uid,"resourceVersion":"1","labels":labels,"annotations":{"webdock.dev/revision":"1","webdock.dev/mail-generation":"1","webdock.dev/mail-profile":manifests::profile(execution.claim).unwrap()}}})).unwrap()
    }
    #[tokio::test]
    async fn legacy_labels_require_reviewed_parent_and_resource_uids() {
        let claim = claim();
        let mut runtime = runtime(&claim);
        let execution = Execution::new(&runtime, &claim, &Authority);
        let uid = uuid::Uuid::new_v4().to_string();
        let namespace = object(&execution, Kind::Namespace, &uid, false);
        assert!(matches!(
            execution.owner(Kind::Namespace, &namespace),
            Err(ExecutionError::Ownership)
        ));
        let deployment_uid = uuid::Uuid::new_v4().to_string();
        runtime.config.retained.insert(
            "123".into(),
            RetainedBinding {
                namespace_uid: uid,
                deployment_uid: deployment_uid.clone(),
                service_uid: uuid::Uuid::new_v4().to_string(),
                volume_uid: uuid::Uuid::new_v4().to_string(),
                claim_uid: uuid::Uuid::new_v4().to_string(),
                bootstrap_secret: None,
                policies: BTreeMap::new(),
            },
        );
        let execution = Execution::new(&runtime, &claim, &Authority);
        execution.owner(Kind::Namespace, &namespace).unwrap();
        let deployment = object(&execution, Kind::Deployment, &deployment_uid, false);
        execution.owner(Kind::Deployment, &deployment).unwrap();
        let wrong = object(
            &execution,
            Kind::Deployment,
            &uuid::Uuid::new_v4().to_string(),
            false,
        );
        assert!(execution.owner(Kind::Deployment, &wrong).is_err());
    }
    #[tokio::test]
    async fn native_resource_replacement_and_checkpoint_uid_changes_are_denied() {
        let mut claim = claim();
        let runtime = runtime(&claim);
        let execution = Execution::new(&runtime, &claim, &Authority);
        let namespace = object(
            &execution,
            Kind::Namespace,
            &uuid::Uuid::new_v4().to_string(),
            true,
        );
        execution.owner(Kind::Namespace, &namespace).unwrap();
        let mut replaced = namespace.clone();
        replaced.metadata.uid = Some(uuid::Uuid::new_v4().to_string());
        assert!(execution.owner(Kind::Namespace, &replaced).is_err());
        claim.checkpoint = Some(MailCheckpoint {
            lease: claim.lease.clone(),
            runtime_id: uuid::Uuid::new_v4().to_string(),
            material_sha256: claim.spec.material_sha256.clone(),
        });
        let execution = Execution::new(&runtime, &claim, &Authority);
        let deployment = object(
            &execution,
            Kind::Deployment,
            &uuid::Uuid::new_v4().to_string(),
            true,
        );
        assert!(execution.owner(Kind::Deployment, &deployment).is_err());
    }
    #[tokio::test]
    async fn creation_handoff_is_scoped_to_current_operation_and_refuses_foreign_field_ownership() {
        let claim = claim();
        let runtime = runtime(&claim);
        let execution = Execution::new(&runtime, &claim, &Authority);
        let mut current = object(
            &execution,
            Kind::Namespace,
            &uuid::Uuid::new_v4().to_string(),
            true,
        );
        let annotations = current.metadata.annotations.as_mut().unwrap();
        annotations.insert(
            "webdock.dev/mail-creation-operation".into(),
            claim.spec.operation_id.clone(),
        );
        annotations.insert("webdock.dev/mail-creation-generation".into(), "1".into());
        annotations.insert("webdock.dev/mail-creation-revision".into(), "1".into());
        current.metadata.managed_fields=Some(serde_json::from_value(json!([{"manager":"webdock-mail-create","operation":"Update","apiVersion":"v1","fieldsType":"FieldsV1","fieldsV1":{"f:metadata":{"f:labels":{"f:security.webdock.dev/ready":{}}}}}])).unwrap());
        let desired = json!({"metadata":{"labels":{"security.webdock.dev/ready":"true"}}});
        assert!(creation_handoff(&current, &desired, &claim).unwrap());
        let foreign=serde_json::from_value(json!({"manager":"operator","operation":"Apply","apiVersion":"v1","fieldsType":"FieldsV1","fieldsV1":{"f:metadata":{"f:labels":{"f:security.webdock.dev/ready":{}}}}})).unwrap();
        current
            .metadata
            .managed_fields
            .as_mut()
            .unwrap()
            .push(foreign);
        assert!(matches!(
            creation_handoff(&current, &desired, &claim),
            Err(ExecutionError::Ownership)
        ));
        current.metadata.managed_fields.as_mut().unwrap().pop();
        let installation = current
            .metadata
            .labels
            .as_mut()
            .unwrap()
            .remove("webdock.dev/mail-installation")
            .unwrap();
        assert!(!creation_handoff(&current, &desired, &claim).unwrap());
        current
            .metadata
            .labels
            .as_mut()
            .unwrap()
            .insert("webdock.dev/mail-installation".into(), installation);
        current
            .metadata
            .annotations
            .as_mut()
            .unwrap()
            .insert("webdock.dev/mail-creation-generation".into(), "2".into());
        assert!(creation_handoff(&current, &desired, &claim).is_err());
        current
            .metadata
            .annotations
            .as_mut()
            .unwrap()
            .insert("webdock.dev/mail-creation-generation".into(), "1".into());
        current.metadata.annotations.as_mut().unwrap().insert(
            "webdock.dev/mail-creation-operation".into(),
            "different-operation".into(),
        );
        assert!(creation_handoff(&current, &desired, &claim).is_err());
    }
    #[tokio::test]
    async fn recovery_removal_is_conditional_and_refuses_foreign_environment_ownership() {
        let claim = claim();
        let runtime = runtime(&claim);
        let execution = Execution::new(&runtime, &claim, &Authority);
        let mut current = object(
            &execution,
            Kind::Deployment,
            &uuid::Uuid::new_v4().to_string(),
            true,
        );
        let patch = recovery_removal(&current).unwrap();
        assert_eq!(patch["metadata"]["uid"], json!(current.metadata.uid));
        assert_eq!(
            patch["metadata"]["resourceVersion"],
            json!(current.metadata.resource_version)
        );
        assert!(patch["spec"].get("replicas").is_none());
        assert!(patch["spec"]["template"].get("metadata").is_none());
        let env = &patch["spec"]["template"]["spec"]["containers"][0]["env"];
        assert_eq!(
            env,
            &json!([{"name":"STALWART_RECOVERY_MODE","$patch":"delete"},{"name":"STALWART_RECOVERY_ADMIN","$patch":"delete"}])
        );
        reject_foreign_fields(&current, &patch).unwrap();
        current.metadata.managed_fields = Some(serde_json::from_value(json!([{"manager":"operator","operation":"Apply","apiVersion":"apps/v1","fieldsType":"FieldsV1","fieldsV1":{"f:spec":{"f:template":{"f:spec":{"f:containers":{"k:{\"name\":\"app\"}":{"f:env":{}}}}}}}}])).unwrap());
        assert!(matches!(
            reject_foreign_fields(&current, &patch),
            Err(ExecutionError::Ownership)
        ));
    }
    #[tokio::test]
    async fn retained_service_udp_on_unprobed_port_is_denied() {
        let claim = claim();
        let runtime = runtime(&claim);
        let execution = Execution::new(&runtime, &claim, &Authority);
        let objects = mail_objects(
            &claim,
            &runtime.config,
            &runtime.storage.1,
            MailPhase::Normal,
        )
        .unwrap();
        let desired = serde_json::to_value(objects.service).unwrap();
        let mut current: DynamicObject = serde_json::from_value(desired.clone()).unwrap();
        current.data["spec"]["ports"][1]["protocol"] = json!("UDP");
        assert!(matches!(
            execution.validate_existing(Kind::Service, &current, &desired),
            Err(ExecutionError::Ownership)
        ));
    }
    #[test]
    fn pv_omitted_empty_storage_class_is_equivalent_but_foreign_class_is_not() {
        let wanted = json!({"storageClassName":"","local":{"path":"/owned/data"}});
        let omitted = json!({"local":{"path":"/owned/data"}});
        assert!(storage_spec_matches(Kind::Volume, &omitted, &wanted));
        assert!(!storage_spec_matches(Kind::Claim, &omitted, &wanted));
        assert!(!storage_spec_matches(
            Kind::Volume,
            &json!({"storageClassName":"foreign","local":{"path":"/owned/data"}}),
            &wanted
        ));
    }
    #[test]
    fn replicas_apply_preserves_legacy_template_markers_without_claiming_other_managers_annotation()
    {
        let mut desired = json!({"spec":{"replicas":0,"template":{"metadata":{"annotations":{"webdock.dev/mail-phase":"normal"}},"spec":{"containers":[{"env":[{"name":"STALWART_PUBLIC_URL","value":"https://mail-123.example.test"}]}]}}}});
        let mut current = desired.clone();
        current["spec"]["replicas"] = 1.into();
        current["spec"]["template"]["metadata"]["annotations"] = json!({"webdock.dev/revision":"7","webdock.dev/operation":"800","webdock.dev/submission-config":"provider-owned"});
        preserve_template_annotations(&mut desired, &current);
        assert_eq!(
            desired["spec"]["template"]["metadata"]["annotations"],
            json!({"webdock.dev/revision":"7","webdock.dev/operation":"800"})
        );
        assert_eq!(desired["spec"]["replicas"], 0);
    }
}
