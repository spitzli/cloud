use crate::{
    model::*,
    provider::Provider,
    store::{Fence, Store},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};
use uuid::Uuid;
use webdock_plugin_kit::seal::Sealer;
type Result<T> = std::result::Result<T, &'static str>;
pub struct Engine {
    pub(crate) store: Store,
    sealer: Sealer,
    pub(crate) provider: Arc<dyn Provider>,
}
impl Engine {
    pub fn new(store: Store, sealer: Sealer, provider: Arc<dyn Provider>) -> Self {
        Self {
            store,
            sealer,
            provider,
        }
    }
    pub fn store(&self) -> &Store {
        &self.store
    }
    pub async fn execute(
        &self,
        customer: &str,
        project: &str,
        action: &str,
        input: Value,
    ) -> Result<Value> {
        tokio::time::timeout(
            Duration::from_secs(45),
            self.execute_inner(customer, project, action, input),
        )
        .await
        .map_err(|_| {
            if action.starts_with("objects.") {
                "Data plan deadline"
            } else {
                "Operation deadline; reconciliation required"
            }
        })?
    }
    async fn execute_inner(
        &self,
        customer: &str,
        project: &str,
        action: &str,
        input: Value,
    ) -> Result<Value> {
        if action == "objects.observe" {
            return serde_json::to_value(self.observe_object(customer, project, input).await?)
                .map_err(|_| "Object metadata encoding failed");
        }
        if let Some(direction) = match action {
            "objects.upload" => Some(webdock_plugin_protocol::storage::StorageDirection::Upload),
            "objects.download" => {
                Some(webdock_plugin_protocol::storage::StorageDirection::Download)
            }
            _ => None,
        } {
            return serde_json::to_value(
                self.prepare_data(customer, project, direction, input)
                    .await?,
            )
            .map_err(|_| "Data plan encoding failed");
        }
        validate(action, &input)?;
        if action.starts_with("lifecycle.") {
            self.provider
                .validate_lifecycle(&policy(&input)?)
                .map_err(|_| "Requested lifecycle setting is unavailable on this backend")?;
        }
        let mut fence = self.store.lock().await?;
        let read = matches!(
            action,
            "buckets.view" | "credentials.reveal" | "lifecycle.preview"
        );
        let request = if read {
            None
        } else {
            Some(uuid(&input, "request_id")?)
        };
        let fingerprint = hex::encode(Sha256::digest(
            serde_json::to_vec(&json!([customer, project, action, input]))
                .map_err(|_| "Invalid input")?,
        ));
        if let Some(id) = request
            && let Some(result) = fence.replay(&id, &fingerprint).await?
        {
            if action == "buckets.create" {
                let resource = fence
                    .get(&uuid(&result["resource"], "id")?)
                    .await?
                    .ok_or("Resource missing")?;
                if resource.state == "deleted" {
                    return Err("Resource was deleted");
                }
            }
            return Ok(result);
        }
        let mut r = if action == "buckets.create" {
            let id = Uuid::new_v4();
            Resource {
                binding: Binding {
                    id,
                    customer: customer.into(),
                    project: project.into(),
                    environment: text(&input, "environment")?.into(),
                    backend: self.provider.backend().into(),
                    bucket: format!("wd-{}", id.simple()),
                    user: format!("wd-{}", id.simple()),
                },
                name: text(&input, "name")?.into(),
                revision: 0,
                state: "creating".into(),
                provisioned: false,
                credentials: vec![],
                observed: None,
                inventory: None,
                pending: None,
                executing: false,
                returned_key: None,
            }
        } else {
            let r = fence
                .get(&uuid(&input, "resource_id")?)
                .await?
                .ok_or("Resource not found")?;
            if r.binding.customer != customer
                || r.binding.project != project
                || r.binding.backend != self.provider.backend()
            {
                return Err("Resource not found");
            }
            r
        };
        if r.state == "deleted" {
            return Err("Resource was deleted");
        }
        if action == "buckets.view" {
            r.observed = fence.provider(self.provider.observe(&r.binding)).await.ok();
            if r.observed.as_ref().is_some_and(|o| !o.exists || !o.owned) {
                r.state = "needs-reconciliation".into();
            }
            fence.save(&r).await?;
            return Ok(self.view(&r));
        }
        if r.executing {
            return Err(
                "Previous execution may still have external effects; private operator recovery required",
            );
        }
        if action == "lifecycle.preview" {
            let policy = policy(&input)?;
            return Ok(
                json!({"policy":policy,"destructive":!policy.empty(),"scope":"all objects and versions in this bucket","estimate":"No deletion count or billing guarantee; concurrent writes can change the result","applied":false}),
            );
        }
        if action == "credentials.reveal" {
            if r.state != "active" || r.pending.is_some() {
                return Err("Credentials unavailable");
            }
            self.owned(&fence, &r).await?;
            let key = text(&input, "access_key")?;
            let c = r
                .credentials
                .iter()
                .find(|c| c.access_key == key)
                .ok_or("Credential not found")?;
            if !fence
                .provider(self.provider.keys(&r.binding))
                .await
                .map_err(provider_error)?
                .contains(&c.access_key)
            {
                return Err("Credential revoked externally");
            }
            let secret = self
                .sealer
                .open(&secret_identity(&r.binding, key), &c.sealed)
                .ok_or("Credential identity mismatch")?;
            return Ok(
                json!({"access_key":key,"secret_key":secret,"endpoint":self.provider.endpoint(),"bucket":r.binding.bucket,"authority":"Permanent project application credentials"}),
            );
        }
        if action != "buckets.create" && input["revision"].as_u64() != Some(r.revision) {
            return Err("Stale resource revision");
        }
        if action != "buckets.reconcile" && r.pending.is_some() {
            return Err("Resource needs reconciliation");
        }
        preconditions(&r, action, &input)?;
        // Validation, ownership observations and empty checks precede the durable effect intent.
        if action != "buckets.create" {
            self.owned(&fence, &r).await?;
        }
        if matches!(action, "credentials.create" | "credentials.rotate") {
            let remote = fence
                .provider(self.provider.keys(&r.binding))
                .await
                .map_err(provider_error)?;
            if remote
                .iter()
                .any(|k| !r.credentials.iter().any(|c| &c.access_key == k))
            {
                return Err("Unknown external keys; reconcile first");
            }
            if remote.len() >= 2 {
                return Err("Two-key limit; explicitly revoke an old key first");
            }
        }
        if action == "buckets.reconcile" {
            if matches!(
                r.pending.as_deref(),
                Some("credentials.create" | "credentials.rotate")
            ) {
                let keys = fence
                    .provider(self.provider.keys(&r.binding))
                    .await
                    .map_err(provider_error)?;
                let returned = r
                    .returned_key
                    .as_ref()
                    .is_some_and(|key| r.credentials.iter().any(|c| &c.access_key == key));
                if !returned
                    && !keys
                        .iter()
                        .any(|key| !r.credentials.iter().any(|c| &c.access_key == key))
                {
                    return Err(
                        "Ambiguous key creation has no settled outcome; private operator reconciliation required",
                    );
                }
            }
            if matches!(
                r.pending.as_deref(),
                Some("versioning.set" | "lifecycle.set" | "buckets.purge" | "buckets.delete")
            ) {
                return Err(
                    "Interrupted provider configuration or deletion requires private operator reconciliation",
                );
            }
        }
        if action == "buckets.delete" {
            if !self.verified_empty(&fence, &r.binding).await? {
                return Err("Bucket is not verified empty; purge separately");
            }
            if !fence
                .provider(self.provider.keys(&r.binding))
                .await
                .map_err(provider_error)?
                .is_empty()
            {
                return Err("Revoke all keys first");
            }
        }
        let request = request.ok_or("Request id required")?;
        let old_state = r.state.clone();
        r.revision = r.revision.checked_add(1).ok_or("Revision exhausted")?;
        r.pending = Some(action.into());
        r.executing = true;
        r.returned_key = None;
        r.state = "needs-reconciliation".into();
        fence.begin(&request, &fingerprint, &r).await?;
        r.state = old_state;
        let result = self.apply(&fence, &mut r, action, &input).await;
        match result {
            Ok(extra) => {
                r.pending = None;
                r.executing = false;
                r.returned_key = None;
                let result = if action == "buckets.create" {
                    json!({"resource":{"id":r.binding.id,"name":r.name},"revision":r.revision})
                } else {
                    let mut view = self.view(&r);
                    view.as_object_mut()
                        .ok_or("Invalid result")?
                        .extend(extra.as_object().ok_or("Invalid result")?.clone());
                    view
                };
                fence.finish(&request, &r, &result).await?;
                Ok(result)
            }
            Err(error) => {
                // Never clear the execution marker using a replacement SQL session.
                fence.check().await?;
                r.executing = false;
                if action == "usage.inventory" {
                    // A metadata scan has no external mutation to reconcile. Drop invalid cursors.
                    r.pending = None;
                    r.inventory = None;
                } else {
                    r.state = "needs-reconciliation".into();
                    r.observed = None;
                }
                fence.save(&r).await?;
                Err(error)
            }
        }
    }
    async fn verified_empty(&self, fence: &Fence, b: &Binding) -> Result<bool> {
        let mut cursor = None;
        for _ in 0..3 {
            let mut page = fence
                .provider(self.provider.inventory(b, cursor.as_ref()))
                .await
                .map_err(provider_error)?;
            cursor = page.next.take();
            if !page.empty() {
                return Ok(false);
            }
            if cursor.is_none() {
                return Ok(true);
            }
        }
        Ok(false)
    }
    async fn owned(&self, fence: &Fence, r: &Resource) -> Result<Observation> {
        let o = fence
            .provider(self.provider.observe(&r.binding))
            .await
            .map_err(provider_error)?;
        if !o.exists || !o.owned {
            return Err("Provider ownership not verified");
        }
        Ok(o)
    }
    async fn apply(
        &self,
        fence: &Fence,
        r: &mut Resource,
        action: &str,
        input: &Value,
    ) -> Result<Value> {
        let p = &self.provider;
        let b = &r.binding;
        let mut extra = json!({});
        match action {
            "buckets.create" => {
                fence
                    .provider(p.provision(b))
                    .await
                    .map_err(provider_error)?;
                r.provisioned = true;
                r.state = "active".into();
            }
            "credentials.create" | "credentials.rotate" => {
                let c = fence
                    .provider(p.create_key(b))
                    .await
                    .map_err(provider_error)?;
                if c.access_key.is_empty()
                    || c.secret_key.is_empty()
                    || r.credentials.iter().any(|v| v.access_key == c.access_key)
                {
                    return Err("Provider credential response invalid");
                }
                let sealed = self
                    .sealer
                    .seal(&secret_identity(b, &c.access_key), &c.secret_key);
                extra = json!({"access_key":c.access_key,"overlap":!r.credentials.is_empty(),"old_key_revocation":"explicit"});
                r.credentials.push(Credential {
                    access_key: c.access_key,
                    sealed,
                });
                r.returned_key = r.credentials.last().map(|c| c.access_key.clone());
                // Persist creation-only secrets and successful-return evidence before observation.
                let mut evidence = r.clone();
                evidence.state = "needs-reconciliation".into();
                fence.save(&evidence).await?;
            }
            "credentials.revoke" => {
                let key = text(input, "access_key")?;
                fence
                    .provider(p.revoke_key(b, key))
                    .await
                    .map_err(provider_error)?;
                if fence
                    .provider(p.keys(b))
                    .await
                    .map_err(provider_error)?
                    .iter()
                    .any(|k| k == key)
                {
                    return Err("Credential revocation not observed");
                }
                r.credentials.retain(|c| c.access_key != key);
            }
            "buckets.deactivate" | "buckets.reconcile" => {
                fence
                    .provider(p.set_lifecycle(b, None))
                    .await
                    .map_err(provider_error)?;
                // The provider namespace was created by this resource, never a customer-selected user.
                for key in fence.provider(p.keys(b)).await.map_err(provider_error)? {
                    fence
                        .provider(p.revoke_key(b, &key))
                        .await
                        .map_err(provider_error)?;
                }
                if !fence
                    .provider(p.keys(b))
                    .await
                    .map_err(provider_error)?
                    .is_empty()
                {
                    return Err("Credential revocation not observed");
                }
                r.credentials.clear();
                r.state = "inactive".into();
            }
            "buckets.reactivate" => {
                r.state = "active".into();
            }
            "versioning.set" => {
                fence
                    .provider(
                        p.set_versioning(
                            b,
                            input["enabled"].as_bool().ok_or("Invalid versioning")?,
                        ),
                    )
                    .await
                    .map_err(provider_error)?;
            }
            "lifecycle.set" => {
                let policy = policy(input)?;
                fence
                    .provider(p.set_lifecycle(b, if policy.empty() { None } else { Some(&policy) }))
                    .await
                    .map_err(provider_error)?;
            }
            "usage.inventory" => {
                let inventory = r.inventory.get_or_insert_with(|| Inventory {
                    started_at: now(),
                    ..Default::default()
                });
                if inventory.complete {
                    *inventory = Inventory {
                        started_at: now(),
                        ..Default::default()
                    };
                }
                let page = match fence
                    .provider(p.inventory(b, inventory.totals.next.as_ref()))
                    .await
                {
                    Ok(page) => page,
                    Err(ProviderError::ScanInvalidated) => {
                        r.inventory = None;
                        return Ok(json!({"scan_reset":true}));
                    }
                    Err(error) => return Err(provider_error(error)),
                };
                macro_rules! add { ($($f:ident),*) => { $(inventory.totals.$f=inventory.totals.$f.checked_add(page.$f).ok_or("Inventory overflow")?;)* }; }
                add!(
                    current_bytes,
                    noncurrent_bytes,
                    multipart_bytes,
                    current_objects,
                    noncurrent_versions,
                    delete_markers,
                    multipart_uploads
                );
                inventory.complete = page.next.is_none();
                inventory.totals.next = page.next;
                inventory.observed_at = now();
            }
            "buckets.purge" => {
                let complete = fence.provider(p.purge(b)).await.map_err(provider_error)?;
                r.inventory = None;
                extra = json!({"purge_complete":complete});
            }
            "buckets.delete" => {
                fence
                    .provider(p.delete_empty(b))
                    .await
                    .map_err(provider_error)?;
                r.state = "deleted".into();
                r.observed = None;
                r.credentials.clear();
                return Ok(json!({"deleted":true}));
            }
            _ => return Err("Action unavailable"),
        }
        let observed = self.owned(fence, r).await?;
        if matches!(
            action,
            "buckets.deactivate" | "buckets.reconcile" | "buckets.reactivate"
        ) && observed.lifecycle.is_some()
        {
            return Err("Destructive lifecycle suspension not observed");
        }
        if action == "versioning.set" && Some(observed.versioning) != input["enabled"].as_bool() {
            return Err("Versioning not observed");
        }
        if action == "lifecycle.set" {
            let policy = policy(input)?;
            if observed.lifecycle.as_ref() != if policy.empty() { None } else { Some(&policy) } {
                return Err("Lifecycle not observed");
            }
        }
        r.observed = Some(observed);
        Ok(extra)
    }
    fn view(&self, r: &Resource) -> Value {
        json!({"resource_id":r.binding.id,"name":r.name,"environment":r.binding.environment,"backend":r.binding.backend,"bucket":r.binding.bucket,"endpoint":self.provider.endpoint(),"revision":r.revision,"state":r.state,"pending":r.pending,"observed":r.observed,"credentials":r.credentials.iter().map(|c|&c.access_key).collect::<Vec<_>>(),"inventory":r.inventory.as_ref().map(|i|json!({"current_bytes":i.totals.current_bytes,"noncurrent_bytes":i.totals.noncurrent_bytes,"multipart_bytes":i.totals.multipart_bytes,"current_objects":i.totals.current_objects,"noncurrent_versions":i.totals.noncurrent_versions,"delete_markers":i.totals.delete_markers,"multipart_uploads":i.totals.multipart_uploads,"complete":i.complete,"started_at":i.started_at,"observed_at":i.observed_at,"consistent_snapshot":false})),"quota":"observed-soft","binary_transfer":"core-authorized","browser_membership_download":"core-authorized"})
    }
}
fn provider_error(error: ProviderError) -> &'static str {
    match error {
        ProviderError::ScanInvalidated => "Inventory changed; start a new scan",
        ProviderError::FenceLost => {
            "Metadata execution fence lost; private operator recovery required"
        }
        _ => "Provider outcome unknown; reconciliation required",
    }
}
fn text<'a>(input: &'a Value, key: &str) -> Result<&'a str> {
    input[key]
        .as_str()
        .filter(|v| !v.is_empty() && v.len() <= 128 && !v.chars().any(char::is_control))
        .ok_or("Invalid input")
}
fn uuid(input: &Value, key: &str) -> Result<Uuid> {
    Uuid::parse_str(text(input, key)?).map_err(|_| "Invalid id")
}
fn policy(input: &Value) -> Result<Lifecycle> {
    serde_json::from_value::<Lifecycle>(input["policy"].clone())
        .ok()
        .filter(Lifecycle::validate)
        .ok_or("Invalid lifecycle")
}
fn secret_identity(b: &Binding, key: &str) -> Uuid {
    let hash =
        Sha256::digest(serde_json::to_vec(&json!([b, key])).expect("binding is serializable"));
    let mut id = [0; 16];
    id.copy_from_slice(&hash[..16]);
    Uuid::from_bytes(id)
}
fn preconditions(r: &Resource, action: &str, input: &Value) -> Result<()> {
    if action == "buckets.reconcile" {
        if !r.provisioned {
            return Err(
                "Interrupted provisioning requires private operator recovery; never adopt collisions",
            );
        }
        return Ok(());
    }
    if r.state == "needs-reconciliation" {
        return Err("Resource needs reconciliation");
    }
    if matches!(
        action,
        "credentials.create" | "credentials.rotate" | "versioning.set" | "lifecycle.set"
    ) && r.state != "active"
    {
        return Err("Resource inactive");
    }
    if action == "credentials.revoke"
        && !r
            .credentials
            .iter()
            .any(|c| Some(c.access_key.as_str()) == input["access_key"].as_str())
    {
        return Err("Credential not found");
    }
    if action == "buckets.reactivate" && r.state != "inactive" {
        return Err("Resource must be inactive");
    }
    if matches!(action, "buckets.purge" | "buckets.delete")
        && (r.state != "inactive"
            || input["confirm"].as_str() != Some(r.binding.id.to_string().as_str()))
    {
        return Err("Inactive resource and exact resource id confirmation required");
    }
    Ok(())
}
fn validate(action: &str, input: &Value) -> Result<()> {
    let extra: &[&str] = match action {
        "buckets.create" => &["name", "environment"],
        "buckets.view" | "buckets.deactivate" | "buckets.reactivate" | "buckets.reconcile"
        | "credentials.create" | "credentials.rotate" | "usage.inventory" => &[],
        "credentials.reveal" | "credentials.revoke" => &["access_key"],
        "versioning.set" => &["enabled"],
        "lifecycle.set" | "lifecycle.preview" => &["policy"],
        "buckets.purge" | "buckets.delete" => &["confirm"],
        _ => return Err("Action unavailable"),
    };
    let read = matches!(
        action,
        "buckets.view" | "credentials.reveal" | "lifecycle.preview"
    );
    let base: &[&str] = if action == "buckets.create" {
        &["request_id"]
    } else if read {
        &["resource_id"]
    } else {
        &["resource_id", "request_id", "revision"]
    };
    let map = input.as_object().ok_or("Invalid input")?;
    if map.len() != base.len() + extra.len()
        || map
            .keys()
            .any(|k| !base.contains(&k.as_str()) && !extra.contains(&k.as_str()))
    {
        return Err("Invalid input fields");
    }
    for k in base.iter().chain(extra) {
        if !map.contains_key(*k) {
            return Err("Missing input field");
        }
    }
    if action == "buckets.create" {
        for key in ["name", "environment"] {
            let s = text(input, key)?;
            if s.trim() != s || s.len() > 63 {
                return Err("Invalid label");
            }
        }
    }
    if action.starts_with("lifecycle.") {
        policy(input)?;
    }
    if action == "versioning.set" && !input["enabled"].is_boolean() {
        return Err("Invalid versioning");
    }
    Ok(())
}
