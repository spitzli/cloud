//! Platform-owned exact Ingress routes. Customer input cannot select issuer, class or backend.
use super::*;
pub use crate::config::PublicRoutingConfig;
use k8s_openapi::api::networking::v1::{Ingress, IngressClass};
use kube::api::{DeleteParams, Preconditions};

pub fn ingress(spec: &RuntimeSpec, config: &PublicRoutingConfig) -> Result<Ingress> {
    spec.validate()?;
    config
        .validate()
        .map_err(|_| ExecutionError::Invalid("public routing configuration"))?;
    let route = spec
        .hosting_binding
        .as_ref()
        .and_then(|b| b.public_route.as_ref())
        .ok_or(ExecutionError::Ownership)?;
    route.validate()?;
    let label = route
        .hostname
        .strip_suffix(&format!(".{}", config.domain))
        .ok_or(ExecutionError::Ownership)?;
    if label.is_empty() || label.contains('.') {
        return Err(ExecutionError::Ownership);
    }
    let mut labels = crate::legacy::owner_labels(spec, true)?;
    labels.insert(
        "webdock.dev/allocation-id".into(),
        route.allocation_id.to_string(),
    );
    serde_json::from_value(json!({"apiVersion":"networking.k8s.io/v1","kind":"Ingress",
        "metadata":{"name":format!("{}-public",crate::legacy::name(spec)),"namespace":crate::legacy::namespace(spec),"labels":labels,
            "annotations":{"webdock.dev/revision":spec.revision.to_string(),"webdock.dev/allocation-revision":route.revision.to_string(),"cert-manager.io/cluster-issuer":config.cluster_issuer,"traefik.ingress.kubernetes.io/router.entrypoints":"websecure","traefik.ingress.kubernetes.io/router.tls":"true"}},
        "spec":{"ingressClassName":config.ingress_class,"tls":[{"hosts":[route.hostname],"secretName":format!("public-{}-tls",route.allocation_id)}],
            "rules":[{"host":route.hostname,"http":{"paths":[{"path":"/","pathType":"Prefix","backend":{"service":{"name":crate::legacy::name(spec),"port":{"number":spec.port}}}}]}}]}}))
        .map_err(|_|ExecutionError::Invalid("public ingress"))
}
/// Only explicitly bounded HTTP01 solvers fit the namespace's reserved certificate allowance.
pub fn compatible_issuer(issuer: &Value, config: &PublicRoutingConfig) -> bool {
    let ready = issuer["metadata"]["generation"]
        .as_u64()
        .is_some_and(|generation| {
            issuer["status"]["conditions"]
                .as_array()
                .is_some_and(|conditions| {
                    conditions.iter().any(|c| {
                        c["type"] == "Ready"
                            && c["status"] == "True"
                            && c["observedGeneration"].as_u64() == Some(generation)
                    })
                })
        });
    if !ready
        || issuer["spec"]["acme"]["server"] != "https://acme-v02.api.letsencrypt.org/directory"
    {
        return false;
    }
    let Some(solvers) = issuer["spec"]["acme"]["solvers"].as_array() else {
        return false;
    };
    !solvers.is_empty() && solvers.iter().all(|solver| {
        let ingress=&solver["http01"]["ingress"];
        if ingress["ingressClassName"]!=config.ingress_class || ingress["serviceType"]!="ClusterIP" {return false}
        let resources=&ingress["podTemplate"]["spec"]["resources"];
        [("cpu",25,200,true),("memory",33_554_432,67_108_864,false),("ephemeral-storage",8_388_608,67_108_864,false)].iter().all(|(name,request_max,limit_max,cpu)| {
            let request=resources["requests"][name].as_str().and_then(|v|parse_quantity(v,*cpu).ok());
            let limit=resources["limits"][name].as_str().and_then(|v|parse_quantity(v,*cpu).ok());
            matches!((request,limit),(Some(request),Some(limit)) if request>0 && request<=*request_max && limit>=request && limit<=*limit_max)
        })
    })
}
fn routing_api(client: Client, spec: &RuntimeSpec) -> Api<Ingress> {
    Api::namespaced(client, &crate::legacy::namespace(spec))
}
fn verify(current: &Ingress, desired: &Ingress) -> Result<()> {
    let labels = current
        .metadata
        .labels
        .as_ref()
        .ok_or(ExecutionError::Ownership)?;
    if desired
        .metadata
        .labels
        .as_ref()
        .unwrap()
        .iter()
        .any(|(k, v)| labels.get(k) != Some(v))
        || current.metadata.deletion_timestamp.is_some()
    {
        return Err(ExecutionError::Ownership);
    }
    let old = current
        .metadata
        .annotations
        .as_ref()
        .and_then(|a| a.get("webdock.dev/revision"))
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or(ExecutionError::Ownership)?;
    let new = desired.metadata.annotations.as_ref().unwrap()["webdock.dev/revision"]
        .parse::<u64>()
        .map_err(|_| ExecutionError::Invalid("route revision"))?;
    if old > new {
        return Err(ExecutionError::Fenced);
    }
    Ok(())
}
impl KubernetesAdapter {
    pub fn with_public_routing(mut self, config: Option<PublicRoutingConfig>) -> Result<Self> {
        if let Some(config) = &config {
            config
                .validate()
                .map_err(|_| ExecutionError::Invalid("public routing configuration"))?
        }
        self.public_routing = config;
        Ok(self)
    }
    pub(super) async fn public_routing_ready(&self) -> bool {
        let Some(config) = &self.public_routing else {
            return false;
        };
        let class: Api<IngressClass> = Api::all(self.client.clone());
        if !class
            .get_opt(&config.ingress_class)
            .await
            .ok()
            .flatten()
            .and_then(|c| c.spec)
            .is_some_and(|s| s.controller.as_deref() == Some("traefik.io/ingress-controller"))
        {
            return false;
        }
        let resource = ApiResource::from_gvk(&GroupVersionKind::gvk(
            "cert-manager.io",
            "v1",
            "ClusterIssuer",
        ));
        let issuers: Api<DynamicObject> = Api::all_with(self.client.clone(), &resource);
        let Some(issuer) = issuers.get_opt(&config.cluster_issuer).await.ok().flatten() else {
            return false;
        };
        serde_json::to_value(&issuer).is_ok_and(|value| compatible_issuer(&value, config))
    }
    pub(super) async fn reconcile_public_route(
        &self,
        command: &Command,
        guard: &LeaseGuard,
        observed: &mut ObservedState,
    ) -> Result<()> {
        if !command
            .spec
            .hosting_binding
            .as_ref()
            .is_some_and(|b| b.public_route.is_some())
        {
            return Ok(());
        }
        let config = self
            .public_routing
            .as_ref()
            .ok_or(ExecutionError::Unsupported("qualified public routing"))?;
        let mut desired = ingress(&command.spec, config)?;
        let name = desired.metadata.name.clone().unwrap();
        let routes = routing_api(self.client.clone(), &command.spec);
        let current = routes
            .get_opt(&name)
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        if let Some(current) = &current {
            verify(current, &desired)?;
        }
        guard.check_for(command).await?;
        if matches!(command.action, Action::Retire | Action::PurgeStorage) {
            if let Some(current) = current {
                routes
                    .delete(
                        &name,
                        &DeleteParams {
                            preconditions: Some(Preconditions {
                                uid: current.metadata.uid,
                                resource_version: current.metadata.resource_version,
                            }),
                            ..Default::default()
                        },
                    )
                    .await
                    .map_err(|_| ExecutionError::Runtime)?;
            }
            return Ok(());
        }
        // Stopping keeps its immutable route reservation and certificate; no new issuance.
        if command.action == Action::Stop || command.spec.replicas == 0 {
            return Ok(());
        }
        if !self.public_routing_ready().await {
            return Err(ExecutionError::Unsupported(
                "ready public ingress and ACME issuer",
            ));
        }
        if let Some(current) = current {
            desired.metadata.resource_version = current.metadata.resource_version;
            desired.metadata.uid = current.metadata.uid;
            routes
                .replace(&name, &PostParams::default(), &desired)
                .await
                .map_err(|_| ExecutionError::Runtime)?;
        } else {
            routes
                .create(&PostParams::default(), &desired)
                .await
                .map_err(|_| ExecutionError::Runtime)?;
        }
        self.observe_public_https(command, guard, observed, true)
            .await
    }
    pub(super) async fn observe_public_https(
        &self,
        command: &Command,
        guard: &LeaseGuard,
        observed: &mut ObservedState,
        wait: bool,
    ) -> Result<()> {
        let Some(route) = command
            .spec
            .hosting_binding
            .as_ref()
            .and_then(|b| b.public_route.as_ref())
        else {
            return Ok(());
        };
        let routes = routing_api(self.client.clone(), &command.spec);
        let name = format!("{}-public", crate::legacy::name(&command.spec));
        let current = routes
            .get_opt(&name)
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        if matches!(command.action, Action::Retire | Action::PurgeStorage) {
            if current.is_some() {
                return Err(ExecutionError::Ownership);
            }
            return Ok(());
        }
        if command.action != Action::Apply || command.spec.replicas == 0 {
            return Ok(());
        }
        let config = self
            .public_routing
            .as_ref()
            .ok_or(ExecutionError::Unsupported("qualified public routing"))?;
        let desired = ingress(&command.spec, config)?;
        let current = current.ok_or(ExecutionError::Ownership)?;
        verify(&current, &desired)?;
        if current.spec != desired.spec {
            return Err(ExecutionError::Ownership);
        }
        let http = reqwest::Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| ExecutionError::Runtime)?;
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(if wait { 150 } else { 0 });
        loop {
            guard.check_for(command).await?;
            let ok = http
                .get(format!(
                    "https://{}{}",
                    route.hostname,
                    command.spec.health_path.as_deref().unwrap_or("/")
                ))
                .send()
                .await
                .is_ok_and(|r| r.status().is_success());
            if ok {
                observed.conditions.push(Condition {
                    kind: "PublicHttpsReady".into(),
                    satisfied: true,
                    reason: "Verified exact-host HTTPS health response".into(),
                });
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(ExecutionError::Runtime);
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn ingress_client_and_object_share_the_existing_project_namespace() {
        let quota = json!({"apps":2,"cpuMillicores":1000,"memoryBytes":1073741824u64,"volumeBytes":0,"ephemeralBytes":1073741824u64,"replicasPerApp":2,"concurrentDeployments":1});
        let spec:RuntimeSpec=serde_json::from_value(json!({"version":1,"installation":"fixture","projectId":"12","resourceId":"13","revision":1,"image":format!("docker.io/library/busybox@sha256:{}","a".repeat(64)),"arguments":[],"cpuMillicores":100,"memoryBytes":67108864,"ephemeralBytes":67108864,"replicas":1,"port":8080,"healthPath":"/","mounts":[],"hostingBinding":{"clusterId":"42","quota":quota,"finalQuota":quota,"publicRoute":{"allocationId":"43","hostname":"owned.spitzli.site","revision":1}}})).unwrap();
        let config = PublicRoutingConfig {
            domain: "spitzli.site".into(),
            ingress_class: "traefik".into(),
            cluster_issuer: "webdock-cms-acme".into(),
        };
        let desired = ingress(&spec, &config).unwrap();
        let client = Client::try_from(Config::new("http://127.0.0.1:1".parse().unwrap())).unwrap();
        let api = routing_api(client, &spec);
        assert_eq!(api.namespace(), desired.metadata.namespace.as_deref());
        assert_eq!(api.namespace(), Some("wd-12"));
    }
    #[test]
    fn foreign_allocation_and_newer_route_cannot_be_overwritten() {
        let desired:Ingress=serde_json::from_value(json!({"metadata":{"labels":{"webdock.dev/project-id":"12","webdock.dev/app-id":"13","webdock.dev/allocation-id":"14"},"annotations":{"webdock.dev/revision":"2"}}})).unwrap();
        let mut foreign = desired.clone();
        foreign
            .metadata
            .labels
            .as_mut()
            .unwrap()
            .insert("webdock.dev/allocation-id".into(), "99".into());
        assert!(verify(&foreign, &desired).is_err());
        let mut newer = desired.clone();
        newer
            .metadata
            .annotations
            .as_mut()
            .unwrap()
            .insert("webdock.dev/revision".into(), "3".into());
        assert!(verify(&newer, &desired).is_err());
        let mut unowned = desired.clone();
        unowned.metadata.labels = None;
        assert!(verify(&unowned, &desired).is_err());
        assert!(verify(&desired, &desired).is_ok());
    }
}
