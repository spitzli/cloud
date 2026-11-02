//! Plugin workloads: Core sends the pinned objects of one unit; this applies them with
//! server-side apply, prunes objects that left the unit, and reports readiness. It checks the
//! pinning again and knows nothing about the products behind the objects.
use kube::{
    Client,
    api::{Api, DeleteParams, DynamicObject, ListParams, Patch, PatchParams, PostParams},
    core::{ApiResource, GroupVersionKind},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use webdock_plugin_protocol::{MILLI, WORKLOAD_KINDS, quantity};

const MANAGER: &str = "webdock-workloads";
const OWNER: &str = "webdock.dev/";

/// What the agent reports for one unit.
pub struct Outcome {
    pub observed: Value,
    pub error: Option<String>,
    pub removed: bool,
}
pub struct WorkloadExecutor {
    client: Client,
    #[cfg(feature = "docker")]
    mail_fence: Option<crate::recovery::effect_fence::MailEffectFence>,
}
fn resource(kind: &str) -> Option<ApiResource> {
    let (api_version, kind) = kind.rsplit_once('/')?;
    let (group, version) = api_version.rsplit_once('/').unwrap_or(("", api_version));
    Some(ApiResource::from_gvk(&GroupVersionKind::gvk(
        group, version, kind,
    )))
}
fn selector(plugin: &str, unit: &str) -> String {
    format!("{OWNER}plugin={plugin},{OWNER}unit={unit}")
}
/// CNPG owns its generated objects; its pods do not inherit Core's pruning labels.
fn pod_selectors(plugin: &str, unit: &str, wanted: &[(String, String)]) -> Vec<String> {
    let mut selectors = vec![selector(plugin, unit)];
    selectors.extend(
        wanted
            .iter()
            .filter(|(kind, _)| kind == "postgresql.cnpg.io/v1/Cluster")
            .map(|(_, name)| format!("cnpg.io/cluster={name}")),
    );
    selectors
}
/// A pod may match both Core and CNPG labels. Count it only once.
async fn selected_pods(
    api: &Api<DynamicObject>,
    selectors: &[String],
) -> Result<Vec<DynamicObject>, kube::Error> {
    let mut pods = BTreeMap::new();
    for selector in selectors {
        for pod in api
            .list(&ListParams::default().labels(selector))
            .await?
            .items
        {
            if let Some(name) = &pod.metadata.name {
                pods.insert(name.clone(), pod);
            }
        }
    }
    Ok(pods.into_values().collect())
}
/// Whether one object is ready, and a short word on why not.
fn readiness(
    kind: &str,
    exists: bool,
    spec: &Value,
    status: &Value,
    generation: Option<i64>,
) -> (bool, String) {
    match kind {
        "apps/v1/Deployment" | "apps/v1/StatefulSet" => {
            let want = spec["replicas"].as_i64().unwrap_or(1);
            let have = status["readyReplicas"].as_i64().unwrap_or(0);
            if want == 0 {
                // Apply acknowledgment precedes controller convergence, including terminating pods.
                let observed = generation.is_some_and(|g| {
                    status["observedGeneration"]
                        .as_i64()
                        .is_some_and(|o| o >= g)
                });
                let stopped = exists
                    && observed
                    && [
                        "replicas",
                        "readyReplicas",
                        "availableReplicas",
                        "updatedReplicas",
                        "currentReplicas",
                        "terminatingReplicas",
                    ]
                    .iter()
                    .all(|field| status[*field].as_i64().unwrap_or(0) == 0);
                return (stopped, if stopped { "Stopped" } else { "Stopping" }.into());
            }
            (exists && have >= want, format!("{have}/{want} ready"))
        }
        "postgresql.cnpg.io/v1/Cluster" => {
            let want = spec["instances"].as_i64().unwrap_or(1);
            let have = status["readyInstances"].as_i64().unwrap_or(0);
            (
                have >= want,
                status["phase"].as_str().unwrap_or("pending").to_owned(),
            )
        }
        "cert-manager.io/v1/Certificate" => {
            let condition = status["conditions"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|c| c["type"] == "Ready");
            (
                condition.is_some_and(|c| c["status"] == "True"),
                condition
                    .and_then(|c| c["message"].as_str())
                    .unwrap_or_default()
                    .to_owned(),
            )
        }
        _ => (exists, String::new()),
    }
}
fn failure(message: impl std::fmt::Display) -> String {
    let text = message.to_string();
    text.chars().take(300).collect()
}
impl WorkloadExecutor {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            #[cfg(feature = "docker")]
            mail_fence: None,
        }
    }
    #[cfg(feature = "docker")]
    pub fn with_mail_fence(
        mut self,
        fence: crate::recovery::effect_fence::MailEffectFence,
    ) -> Self {
        self.mail_fence = Some(fence);
        self
    }
    #[cfg(feature = "docker")]
    pub fn set_mail_fence(&mut self, fence: crate::recovery::effect_fence::MailEffectFence) {
        self.mail_fence = Some(fence);
    }
    #[cfg(feature = "docker")]
    async fn mail_effect(
        &self,
        plugin: &str,
        effect: Value,
    ) -> Result<Option<crate::recovery::effect_fence::MailEffectPermit>, String> {
        if plugin == "mail"
            && let Some(fence) = &self.mail_fence
        {
            return fence
                .enter_effect(effect)
                .await
                .map(Some)
                .map_err(|_| "Mail recovery source effect is fenced".into());
        }
        Ok(None)
    }
    /// Objects must carry the namespace and owner labels Core pinned; anything else is refused.
    fn check(plugin: &str, unit: &str, namespace: &str, objects: &[Value]) -> Result<(), String> {
        if !namespace.starts_with("wd-") {
            return Err("namespace not owned by workloads".into());
        }
        for object in objects {
            let kind = format!(
                "{}/{}",
                object["apiVersion"].as_str().unwrap_or_default(),
                object["kind"].as_str().unwrap_or_default()
            );
            let labels = &object["metadata"]["labels"];
            if !WORKLOAD_KINDS.contains(&kind.as_str())
                || object["metadata"]["namespace"] != namespace
                || labels[format!("{OWNER}plugin")] != plugin
                || labels[format!("{OWNER}unit")] != unit
                || labels[format!("{OWNER}managed-by")] != "webdock"
            {
                return Err(format!("object outside the unit: {kind}"));
            }
        }
        Ok(())
    }
    async fn namespace(&self, namespace: &str, plugin: &str) -> Result<(), String> {
        let api: Api<DynamicObject> = Api::all_with(
            self.client.clone(),
            &ApiResource::from_gvk(&GroupVersionKind::gvk("", "v1", "Namespace")),
        );
        match api.get_opt(namespace).await.map_err(failure)? {
            Some(existing) => {
                if existing
                    .metadata
                    .labels
                    .unwrap_or_default()
                    .get("webdock.dev/managed-by")
                    .map(String::as_str)
                    != Some("webdock")
                {
                    return Err("namespace exists but is not managed by Webdock".into());
                }
                Ok(())
            }
            None => {
                let object: DynamicObject = serde_json::from_value(json!({
                    "apiVersion": "v1", "kind": "Namespace",
                    "metadata": {"name": namespace, "labels": {
                        "webdock.dev/managed-by": "webdock", "webdock.dev/plugin": plugin,
                        "pod-security.kubernetes.io/enforce": "baseline",
                    }},
                }))
                .map_err(failure)?;
                #[cfg(feature = "docker")]
                let effect = self
                    .mail_effect(plugin, json!({"namespace":namespace,"method":"POST"}))
                    .await?;
                api.create(&PostParams::default(), &object)
                    .await
                    .map_err(failure)?;
                #[cfg(feature = "docker")]
                if let Some(effect) = effect {
                    effect.confirmed().map_err(failure)?;
                }
                Ok(())
            }
        }
    }
    /// Every object of the unit that exists now, across the allowed kinds.
    async fn existing(&self, namespace: &str, plugin: &str, unit: &str) -> Vec<(String, String)> {
        let mut found = Vec::new();
        for kind in WORKLOAD_KINDS {
            let Some(resource) = resource(kind) else {
                continue;
            };
            let api: Api<DynamicObject> =
                Api::namespaced_with(self.client.clone(), namespace, &resource);
            // Kinds whose CRD is not installed simply have nothing to list.
            if let Ok(list) = api
                .list(&ListParams::default().labels(&selector(plugin, unit)))
                .await
            {
                found.extend(
                    list.items
                        .into_iter()
                        .filter_map(|o| o.metadata.name)
                        .map(|name| ((*kind).to_owned(), name)),
                );
            }
        }
        found
    }
    async fn delete(
        &self,
        _plugin: &str,
        namespace: &str,
        kind: &str,
        name: &str,
    ) -> Result<(), String> {
        let resource = resource(kind).ok_or("unknown kind")?;
        let api: Api<DynamicObject> =
            Api::namespaced_with(self.client.clone(), namespace, &resource);
        #[cfg(feature = "docker")]
        let effect = self
            .mail_effect(
                _plugin,
                json!({"namespace":namespace,"kind":kind,"name":name,"method":"DELETE"}),
            )
            .await?;
        let result = match api.delete(name, &DeleteParams::background()).await {
            Ok(_) => Ok(()),
            Err(kube::Error::Api(status)) if status.code == 404 => Ok(()),
            Err(error) => Err(failure(error)),
        };
        result?;
        #[cfg(feature = "docker")]
        if let Some(effect) = effect {
            effect.confirmed().map_err(failure)?;
        }
        Ok(())
    }

    pub async fn handle(
        &self,
        plugin: &str,
        unit: &str,
        namespace: &str,
        objects_json: &[u8],
        removing: bool,
    ) -> Outcome {
        if removing {
            let existing = self.existing(namespace, plugin, unit).await;
            for (kind, name) in &existing {
                if let Err(error) = self.delete(plugin, namespace, kind, name).await {
                    return Outcome {
                        observed: json!({"ready": false}),
                        error: Some(error),
                        removed: false,
                    };
                }
            }
            return Outcome {
                observed: json!({"ready": false}),
                error: None,
                removed: existing.is_empty(),
            };
        }
        let objects: Vec<Value> = match serde_json::from_slice(objects_json) {
            Ok(objects) => objects,
            Err(error) => {
                return Outcome {
                    observed: json!({"ready": false}),
                    error: Some(failure(error)),
                    removed: false,
                };
            }
        };
        match self.apply(plugin, unit, namespace, &objects).await {
            Ok(observed) => Outcome {
                observed,
                error: None,
                removed: false,
            },
            Err(error) => Outcome {
                observed: json!({"ready": false}),
                error: Some(error),
                removed: false,
            },
        }
    }
    async fn apply(
        &self,
        plugin: &str,
        unit: &str,
        namespace: &str,
        objects: &[Value],
    ) -> Result<Value, String> {
        Self::check(plugin, unit, namespace, objects)?;
        self.namespace(namespace, plugin).await?;
        let mut wanted = Vec::new();
        for object in objects {
            let kind = format!(
                "{}/{}",
                object["apiVersion"].as_str().unwrap_or_default(),
                object["kind"].as_str().unwrap_or_default()
            );
            let name = object["metadata"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let resource = resource(&kind).ok_or("unknown kind")?;
            let api: Api<DynamicObject> =
                Api::namespaced_with(self.client.clone(), namespace, &resource);
            #[cfg(feature = "docker")]
            let effect = self.mail_effect(plugin,json!({"namespace":namespace,"kind":kind,"name":name,"method":"PATCH","intentSha256":crate::recovery::config::fingerprint(object).map_err(failure)?})).await?;
            api.patch(
                &name,
                &PatchParams::apply(MANAGER).force(),
                &Patch::Apply(object),
            )
            .await
            .map_err(|e| failure(format!("{kind} {name}: {e}")))?;
            #[cfg(feature = "docker")]
            if let Some(effect) = effect {
                effect.confirmed().map_err(failure)?;
            }
            wanted.push((kind, name));
        }
        for (kind, name) in self.existing(namespace, plugin, unit).await {
            if !wanted.contains(&(kind.clone(), name.clone())) {
                self.delete(plugin, namespace, &kind, &name).await?;
            }
        }
        self.observe(namespace, plugin, unit, &wanted).await
    }
    async fn observe(
        &self,
        namespace: &str,
        plugin: &str,
        unit: &str,
        wanted: &[(String, String)],
    ) -> Result<Value, String> {
        let mut states = Vec::new();
        let mut ready = true;
        for (kind, name) in wanted {
            let resource = resource(kind).ok_or("unknown kind")?;
            let api: Api<DynamicObject> =
                Api::namespaced_with(self.client.clone(), namespace, &resource);
            let object = api.get_opt(name).await.map_err(failure)?;
            let status = object
                .as_ref()
                .map(|o| o.data["status"].clone())
                .unwrap_or(Value::Null);
            let spec = object
                .as_ref()
                .map(|o| o.data["spec"].clone())
                .unwrap_or(Value::Null);
            let (is_ready, message) = readiness(
                kind,
                object.is_some(),
                &spec,
                &status,
                object.as_ref().and_then(|o| o.metadata.generation),
            );
            ready &= is_ready;
            states.push(json!({"kind": kind, "name": name, "ready": is_ready, "message": message}));
        }
        // Why pods are not running, in the operator's and customer's words: pull and crash states.
        let pods: Api<DynamicObject> = Api::namespaced_with(
            self.client.clone(),
            namespace,
            &ApiResource::from_gvk(&GroupVersionKind::gvk("", "v1", "Pod")),
        );
        let mut problems = Vec::new();
        let mut logs = Vec::new();
        let selectors = pod_selectors(plugin, unit, wanted);
        if let Ok(list) = selected_pods(&pods, &selectors).await {
            let typed: Api<k8s_openapi::api::core::v1::Pod> =
                Api::namespaced(self.client.clone(), namespace);
            if let Some(name) = list.iter().find_map(|p| p.metadata.name.clone())
                && let Ok(text) = typed
                    .logs(
                        &name,
                        &kube::api::LogParams {
                            tail_lines: Some(100),
                            timestamps: true,
                            ..Default::default()
                        },
                    )
                    .await
            {
                // The newest lines, at most 24 KiB, without control characters.
                let mut budget = 24 * 1024;
                for line in text.lines().rev() {
                    let clean: String = line
                        .chars()
                        .filter(|c| !c.is_control() || *c == '\t')
                        .collect();
                    if clean.len() > budget {
                        break;
                    }
                    budget -= clean.len();
                    logs.push(clean);
                }
                logs.reverse();
            }
            for pod in list {
                for container in pod.data["status"]["containerStatuses"]
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    if let Some(reason) = container["state"]["waiting"]["reason"].as_str()
                        && reason != "ContainerCreating"
                        && reason != "PodInitializing"
                    {
                        problems.push(json!({"container": container["name"], "reason": reason, "restarts": container["restartCount"]}));
                    }
                }
            }
        }
        let mut observed =
            json!({"ready": ready, "objects": states, "problems": problems, "logs": logs});
        if let Some(usage) = self.usage(namespace, &selectors).await {
            observed["usage"] = usage;
        }
        Ok(observed)
    }
    /// What the unit's pods use right now, from the metrics API: CPU in millicores, memory in MiB.
    /// None when the cluster has no metrics; no pods (stopped) use nothing.
    async fn usage(&self, namespace: &str, selectors: &[String]) -> Option<Value> {
        let resource = ApiResource {
            group: "metrics.k8s.io".into(),
            version: "v1beta1".into(),
            api_version: "metrics.k8s.io/v1beta1".into(),
            kind: "PodMetrics".into(),
            plural: "pods".into(),
        };
        let api: Api<DynamicObject> =
            Api::namespaced_with(self.client.clone(), namespace, &resource);
        let list = selected_pods(&api, selectors).await.ok()?;
        let (mut cpu, mut memory) = (0u64, 0u128);
        for pod in &list {
            for container in pod.data["containers"].as_array().into_iter().flatten() {
                cpu = cpu.saturating_add(quantity(&container["usage"]["cpu"], MILLI).unwrap_or(0));
                // Bytes first: rounding each container up to a MiB would overstate small ones.
                memory += u128::from(quantity(&container["usage"]["memory"], (1, 1)).unwrap_or(0));
            }
        }
        Some(json!({"cpu": cpu, "memory": memory.div_ceil(1 << 20)}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "docker")]
    #[tokio::test]
    async fn captured_workload_released_after_fence_uses_real_executor_guard_before_provider_patch()
    {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        async fn provider(
            axum::extract::State(count): axum::extract::State<Arc<AtomicUsize>>,
            request: axum::extract::Request,
        ) -> axum::Json<Value> {
            let patch = request.method() == axum::http::Method::PATCH;
            let namespace = request.uri().path() == "/api/v1/namespaces/wd-mail-c42";
            axum::body::to_bytes(request.into_body(), 65536)
                .await
                .unwrap();
            axum::Json(if patch {
                count.fetch_add(1, Ordering::SeqCst);
                json!({"apiVersion":"apps/v1","kind":"Deployment","metadata":{"name":"engine","namespace":"wd-mail-c42"},"spec":{"replicas":1}})
            } else if namespace {
                json!({"apiVersion":"v1","kind":"Namespace","metadata":{"name":"wd-mail-c42","labels":{"webdock.dev/managed-by":"webdock","webdock.dev/plugin":"mail"}}})
            } else {
                json!({"apiVersion":"v1","kind":"List","metadata":{},"items":[]})
            })
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let patches = Arc::new(AtomicUsize::new(0));
        let router = axum::Router::new()
            .fallback(provider)
            .with_state(patches.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = Client::try_from(kube::Config::new(
            format!("http://{address}").parse().unwrap(),
        ))
        .unwrap();
        let gate = crate::recovery::effect_fence::MailEffectFence::default();
        let executor = WorkloadExecutor::new(client.clone()).with_mail_fence(gate.clone());
        let old = json!({"apiVersion":"apps/v1","kind":"Deployment","metadata":{"name":"engine","namespace":"wd-mail-c42","labels":{"webdock.dev/managed-by":"webdock","webdock.dev/plugin":"mail","webdock.dev/unit":"engine-c42"}},"spec":{"replicas":1}});
        let (release, held) = tokio::sync::oneshot::channel();
        let captured = old.clone();
        let worker = tokio::spawn(async move {
            held.await.unwrap();
            executor
                .apply("mail", "engine-c42", "wd-mail-c42", &[captured])
                .await
        });
        let _drained = gate.close_and_drain().await.unwrap();
        release.send(()).unwrap();
        assert!(worker.await.unwrap().is_err());
        assert_eq!(patches.load(Ordering::SeqCst), 0);
        // A genuinely legacy executor has no guard. Its effect remains possible;
        // Core54 must keep that registered participant undrained until owned stop.
        let legacy = WorkloadExecutor::new(client);
        let _ = legacy
            .apply("mail", "engine-c42", "wd-mail-c42", &[old])
            .await;
        assert_eq!(patches.load(Ordering::SeqCst), 1);
        server.abort();
        let _ = server.await;
    }
    #[cfg(feature = "docker")]
    #[tokio::test]
    async fn lost_remote_patch_response_survives_cancellation_and_blocks_drain_after_restart() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        struct Held {
            submitted: tokio::sync::Notify,
            release: tokio::sync::Notify,
            applied: AtomicUsize,
        }
        async fn accepted_effect(accepted: Arc<Held>, complete: tokio::sync::oneshot::Sender<()>) {
            accepted.submitted.notify_one();
            if tokio::time::timeout(
                std::time::Duration::from_secs(10),
                accepted.release.notified(),
            )
            .await
            .is_ok()
            {
                accepted.applied.fetch_add(1, Ordering::SeqCst);
            }
            let _ = complete.send(());
        }
        async fn provider(
            axum::extract::State(state): axum::extract::State<Arc<Held>>,
            request: axum::extract::Request,
        ) -> axum::Json<Value> {
            let patch = request.method() == axum::http::Method::PATCH;
            let namespace = request.uri().path() == "/api/v1/namespaces/wd-mail-c42";
            axum::body::to_bytes(request.into_body(), 65536)
                .await
                .unwrap();
            if patch {
                let accepted = state.clone();
                let (complete, done) = tokio::sync::oneshot::channel();
                // The provider owns accepted work independently of an HTTP client that disappears.
                tokio::spawn(accepted_effect(accepted, complete));
                let _ = done.await;
            }
            axum::Json(if namespace {
                json!({"apiVersion":"v1","kind":"Namespace","metadata":{"name":"wd-mail-c42","labels":{"webdock.dev/managed-by":"webdock"}}})
            } else {
                json!({"apiVersion":"apps/v1","kind":"Deployment","metadata":{"name":"engine","namespace":"wd-mail-c42"},"spec":{"replicas":1}})
            })
        }
        let state = Arc::new(Held {
            submitted: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            applied: AtomicUsize::new(0),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = axum::Router::new()
            .fallback(provider)
            .with_state(state.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let directory = tempfile::tempdir().unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.path().join("effects.json");
        let gate = crate::recovery::effect_fence::MailEffectFence::open(&path).unwrap();
        let client = Client::try_from(kube::Config::new(
            format!("http://{address}").parse().unwrap(),
        ))
        .unwrap();
        let executor = WorkloadExecutor::new(client).with_mail_fence(gate.clone());
        let old = json!({"apiVersion":"apps/v1","kind":"Deployment","metadata":{"name":"engine","namespace":"wd-mail-c42","labels":{"webdock.dev/managed-by":"webdock","webdock.dev/plugin":"mail","webdock.dev/unit":"engine-c42"}},"spec":{"replicas":1}});
        let worker = tokio::spawn(async move {
            executor
                .apply("mail", "engine-c42", "wd-mail-c42", &[old])
                .await
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            state.submitted.notified(),
        )
        .await
        .unwrap();
        worker.abort();
        let _ = worker.await;
        assert!(
            gate.close_and_drain().await.is_err(),
            "local future cancellation cannot drain a held remote PATCH"
        );
        state.release.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while state.applied.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(state.applied.load(Ordering::SeqCst), 1);
        drop(gate);
        let restarted = crate::recovery::effect_fence::MailEffectFence::open(&path).unwrap();
        assert!(restarted.close_and_drain().await.is_err());
        server.abort();
        let _ = server.await;
    }
    fn service(namespace: &str, unit: &str) -> Value {
        json!({"apiVersion":"v1","kind":"Service","metadata":{"name":"web","namespace":namespace,
               "labels":{"webdock.dev/managed-by":"webdock","webdock.dev/plugin":"hosting","webdock.dev/unit":unit}}})
    }
    #[test]
    fn objects_must_stay_in_their_unit() {
        assert!(
            WorkloadExecutor::check(
                "hosting",
                "a",
                "wd-hosting-1",
                &[service("wd-hosting-1", "a")]
            )
            .is_ok()
        );
        for objects in [
            vec![service("kube-system", "a")],
            vec![service("wd-hosting-1", "b")],
            vec![json!({"apiVersion":"v1","kind":"Pod","metadata":{"namespace":"wd-hosting-1"}})],
        ] {
            assert!(WorkloadExecutor::check("hosting", "a", "wd-hosting-1", &objects).is_err());
        }
        assert!(
            WorkloadExecutor::check(
                "hosting",
                "a",
                "kube-system",
                &[service("kube-system", "a")]
            )
            .is_err()
        );
        assert_eq!(
            resource("traefik.io/v1alpha1/IngressRoute").unwrap().plural,
            "ingressroutes"
        );
        assert_eq!(resource("v1/Service").unwrap().group, "");
    }
    #[test]
    fn zero_replicas_waits_for_the_last_running_instance() {
        for kind in ["apps/v1/Deployment", "apps/v1/StatefulSet"] {
            for status in [
                json!({"observedGeneration":2,"replicas":1,"readyReplicas":0}),
                json!({"observedGeneration":2,"replicas":0,"readyReplicas":1}),
                json!({"observedGeneration":2,"replicas":0,"availableReplicas":1}),
                json!({"observedGeneration":2,"replicas":0,"updatedReplicas":1}),
                json!({"observedGeneration":2,"replicas":0,"terminatingReplicas":1}),
                json!({"observedGeneration":2,"replicas":0,"currentReplicas":1}),
                json!({"observedGeneration":1,"replicas":0}),
                json!({}),
            ] {
                assert!(
                    !readiness(kind, true, &json!({"replicas":0}), &status, Some(2)).0,
                    "{kind}: {status}"
                );
            }
            assert!(
                readiness(
                    kind,
                    true,
                    &json!({"replicas":0}),
                    &json!({"observedGeneration":2,"replicas":0}),
                    Some(2)
                )
                .0
            );
            assert!(
                readiness(
                    kind,
                    true,
                    &json!({"replicas":0}),
                    &json!({"observedGeneration":2}),
                    Some(2)
                )
                .0
            );
            assert!(!readiness(kind, false, &json!({"replicas":0}), &json!({}), Some(2)).0);
        }
    }
    #[test]
    fn readiness_follows_each_kind() {
        let deployment = readiness(
            "apps/v1/Deployment",
            true,
            &json!({"replicas":1}),
            &json!({"readyReplicas":1}),
            None,
        );
        assert!(deployment.0);
        assert!(
            !readiness(
                "apps/v1/Deployment",
                true,
                &json!({"replicas":1}),
                &json!({}),
                None
            )
            .0
        );
        let issued = json!({"conditions":[{"type":"Ready","status":"True"}]});
        let pending = json!({"conditions":[{"type":"Ready","status":"False","message":"Issuing"}]});
        assert!(
            readiness(
                "cert-manager.io/v1/Certificate",
                true,
                &json!({}),
                &issued,
                None
            )
            .0
        );
        let waiting = readiness(
            "cert-manager.io/v1/Certificate",
            true,
            &json!({}),
            &pending,
            None,
        );
        assert_eq!(waiting, (false, "Issuing".to_owned()));
        assert!(readiness("v1/Service", true, &json!({}), &json!({}), None).0);
        assert!(!readiness("v1/Service", false, &json!({}), &json!({}), None).0);
    }
}
