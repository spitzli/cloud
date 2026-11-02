use super::*;
use crate::storage::database::{DatabaseStorageCapability, DatabaseStorageClient};
use k8s_openapi::api::{
    apps::v1::Deployment,
    core::v1::{Node, Pod, Service},
};
use kube::{
    Api, ResourceExt,
    api::{ApiResource, DynamicObject, GroupVersionKind, ListParams},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseCapabilities {
    pub database_version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database_backup_version: Option<u8>,
    pub database_pools: Vec<DatabasePoolObservation>,
    pub database_node_ports: Vec<i32>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabasePoolObservation {
    #[serde(flatten)]
    pub storage: DatabaseStorageCapability,
    pub requested_cpu_millicores: u64,
    pub requested_memory_bytes: u64,
    pub observed_databases: Vec<DatabaseResourceObservation>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseResourceObservation {
    #[serde(rename = "installationID")]
    pub installation_id: String,
    pub cpu_millicores: u64,
    pub memory_bytes: u64,
}
fn node_available(node: &Node) -> bool {
    let Some(spec) = &node.spec else {
        return false;
    };
    if spec.unschedulable == Some(true) || spec.taints.as_ref().is_some_and(|t| !t.is_empty()) {
        return false;
    }
    let Some(conditions) = node.status.as_ref().and_then(|s| s.conditions.as_ref()) else {
        return false;
    };
    conditions
        .iter()
        .any(|c| c.type_ == "Ready" && c.status == "True")
        && !conditions.iter().any(|c| {
            ["MemoryPressure", "DiskPressure", "PIDPressure"].contains(&c.type_.as_str())
                && c.status == "True"
        })
}
fn pool_observation(
    storage: DatabaseStorageCapability,
    pods: &[Pod],
) -> Result<DatabasePoolObservation> {
    let mut observation = DatabasePoolObservation {
        storage,
        requested_cpu_millicores: 0,
        requested_memory_bytes: 0,
        observed_databases: vec![],
    };
    let mut installations = BTreeMap::<String, DatabaseResourceObservation>::new();
    for pod in pods {
        if pod
            .status
            .as_ref()
            .and_then(|s| s.phase.as_deref())
            .is_some_and(|p| matches!(p, "Succeeded" | "Failed"))
        {
            continue;
        }
        let spec = pod.spec.as_ref().ok_or(ExecutionError::Runtime)?;
        if spec
            .node_name
            .as_ref()
            .is_some_and(|n| n != &observation.storage.node)
        {
            continue;
        }
        let installation = pod
            .metadata
            .namespace
            .as_deref()
            .and_then(|n| n.strip_prefix("wd-db-"))
            .filter(|value| id(value));
        // Unscheduled and init containers are included conservatively, as in the retained observer.
        for container in spec
            .containers
            .iter()
            .chain(spec.init_containers.iter().flatten())
        {
            let request = container
                .resources
                .as_ref()
                .and_then(|r| r.requests.as_ref());
            let cpu = crate::kubernetes::parse_quantity(
                request
                    .and_then(|r| r.get("cpu"))
                    .map_or("0", |v| v.0.as_str()),
                true,
            )?;
            let memory = crate::kubernetes::parse_quantity(
                request
                    .and_then(|r| r.get("memory"))
                    .map_or("0", |v| v.0.as_str()),
                false,
            )?;
            observation.requested_cpu_millicores = observation
                .requested_cpu_millicores
                .checked_add(cpu)
                .ok_or(ExecutionError::Runtime)?;
            observation.requested_memory_bytes = observation
                .requested_memory_bytes
                .checked_add(memory)
                .ok_or(ExecutionError::Runtime)?;
            if let Some(id) = installation {
                let item =
                    installations
                        .entry(id.into())
                        .or_insert_with(|| DatabaseResourceObservation {
                            installation_id: id.into(),
                            ..Default::default()
                        });
                item.cpu_millicores = item
                    .cpu_millicores
                    .checked_add(cpu)
                    .ok_or(ExecutionError::Runtime)?;
                item.memory_bytes = item
                    .memory_bytes
                    .checked_add(memory)
                    .ok_or(ExecutionError::Runtime)?;
            }
        }
    }
    observation.observed_databases = installations.into_values().collect();
    if observation.observed_databases.len() > 1000 {
        return Err(ExecutionError::Runtime);
    }
    Ok(observation)
}
impl NativeDatabaseExecutor {
    pub(super) async fn admission_ready(&self, name: &str) -> Result<bool> {
        let mut objects = Vec::new();
        for kind in [
            "ValidatingAdmissionPolicy",
            "ValidatingAdmissionPolicyBinding",
        ] {
            let resource = ApiResource::from_gvk(&GroupVersionKind::gvk(
                "admissionregistration.k8s.io",
                "v1",
                kind,
            ));
            let api: Api<DynamicObject> = Api::all_with(self.client.clone(), &resource);
            let Some(object) = api
                .get_opt(name)
                .await
                .map_err(|_| ExecutionError::Runtime)?
            else {
                return Ok(false);
            };
            objects.push(object);
        }
        Ok(objects[0]
            .metadata
            .labels
            .as_ref()
            .and_then(|l| l.get("webdock.dev/database-boundary-version"))
            .map(String::as_str)
            == Some("1")
            && objects[0].data["status"]["typeChecking"]["expressionWarnings"]
                .as_array()
                .is_none_or(Vec::is_empty)
            && objects[1].data["spec"]["policyName"] == name
            && objects[1].data["spec"]["validationActions"]
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v == "Deny")))
    }
    pub async fn capabilities(&self, nodes: &[String]) -> Result<Option<DatabaseCapabilities>> {
        self.verify_configuration()?;
        let operator: Api<Deployment> = Api::namespaced(self.client.clone(), "cnpg-system");
        let Some(operator) = operator
            .get_opt("cnpg-controller-manager")
            .await
            .map_err(|_| ExecutionError::Runtime)?
        else {
            return Ok(None);
        };
        if operator
            .status
            .as_ref()
            .and_then(|s| s.available_replicas)
            .unwrap_or(0)
            < 1
        {
            return Ok(None);
        }
        for name in ["webdock-database-pods", "webdock-database-services"] {
            if !self.admission_ready(name).await? {
                return Ok(None);
            }
        }
        let node_api: Api<Node> = Api::all(self.client.clone());
        let observed_nodes = node_api
            .list(&ListParams::default().limit(201))
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        let pod_api: Api<Pod> = Api::all(self.client.clone());
        let pods = pod_api
            .list(&ListParams::default().limit(10001))
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        let service_api: Api<Service> = Api::all(self.client.clone());
        let services = service_api
            .list(&ListParams::default().limit(10001))
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        if observed_nodes.items.len() > 200
            || pods.items.len() > 10000
            || services.items.len() > 10000
            || [&observed_nodes.metadata, &pods.metadata, &services.metadata]
                .iter()
                .any(|m| m.continue_.as_ref().is_some_and(|s| !s.is_empty()))
        {
            return Err(ExecutionError::Runtime);
        }
        let available_nodes = observed_nodes
            .items
            .iter()
            .filter(|n| node_available(n))
            .map(ResourceExt::name_any)
            .collect::<BTreeSet<_>>();
        let mut pools = Vec::new();
        let routes = std::iter::once((&self.config.storage_socket, None)).chain(
            self.config
                .additional_storage_pools
                .iter()
                .map(|r| (&r.storage_socket, Some(r))),
        );
        let mut seen_nodes = BTreeSet::new();
        let mut seen_pools = BTreeSet::new();
        let mut seen_installations = BTreeSet::new();
        for (socket, route) in routes {
            let storage = DatabaseStorageClient::new(socket.clone())
                .capability(nodes)
                .await?;
            if route.is_some_and(|r| r.node != storage.node || r.pool_id != storage.pool_id)
                || (route.is_none()
                    && self
                        .config
                        .additional_storage_pools
                        .iter()
                        .any(|r| r.node == storage.node || r.pool_id == storage.pool_id))
                || !seen_nodes.insert(storage.node.clone())
                || !seen_pools.insert(storage.pool_id.clone())
                || storage
                    .allocations
                    .iter()
                    .any(|a| !seen_installations.insert(a.installation_id.to_string()))
            {
                return Err(ExecutionError::Ownership);
            }
            if storage.ready && available_nodes.contains(&storage.node) {
                pools.push(pool_observation(storage, &pods.items)?);
            }
        }
        if pools.is_empty() {
            return Ok(None);
        }
        let ports = services
            .items
            .into_iter()
            .filter_map(|s| s.spec)
            .flat_map(|s| s.ports.unwrap_or_default())
            .filter_map(|p| p.node_port)
            .collect();
        Ok(Some(DatabaseCapabilities {
            database_version: 1,
            database_backup_version: if self.config.backup.is_some()
                && self.verify_backup_runtime().await.is_ok()
            {
                Some(1)
            } else {
                None
            },
            database_pools: pools,
            database_node_ports: ports,
        }))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capacity_accounts_for_unplaced_init_and_foreign_workloads_without_counting_finished_pods() {
        let storage: DatabaseStorageCapability=serde_json::from_value(serde_json::json!({"node":"node-a","poolID":"00000000-0000-4000-8000-000000000001","capacityBytes":1000,"ready":true,"availableBytes":1000,"allocatedBytes":0,"allocations":[]})).unwrap();
        let pod = |namespace: &str, node: Option<&str>, phase: &str| {
            serde_json::from_value::<Pod>(serde_json::json!({"metadata":{"name":"test","namespace":namespace},"spec":{"nodeName":node,"containers":[{"name":"main","resources":{"requests":{"cpu":"100m","memory":"1Mi"}}}],"initContainers":[{"name":"init","resources":{"requests":{"cpu":"50m","memory":"2Mi"}}}]},"status":{"phase":phase}})).unwrap()
        };
        let result = pool_observation(
            storage,
            &[
                pod("wd-db-123", None, "Pending"),
                pod("foreign", Some("node-a"), "Running"),
                pod("wd-db-456", Some("node-b"), "Running"),
                pod("wd-db-789", Some("node-a"), "Succeeded"),
            ],
        )
        .unwrap();
        assert_eq!(result.requested_cpu_millicores, 300);
        assert_eq!(result.requested_memory_bytes, 6 * 1024 * 1024);
        assert_eq!(result.observed_databases.len(), 1);
        assert_eq!(result.observed_databases[0].installation_id, "123");
        assert_eq!(result.observed_databases[0].cpu_millicores, 150);
    }
}
