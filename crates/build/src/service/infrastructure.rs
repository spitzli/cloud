//! Operator-only setup through existing live browser/session authority, never customer Pod inputs.
use super::*;
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum InfrastructureCommand {
    Admission {
        enabled: bool,
        global_slots: u32,
        tenant_slots: u32,
    },
    TrustProject {
        #[serde(rename = "customerID")]
        customer_id: String,
        #[serde(rename = "projectID")]
        project_id: String,
        enabled: bool,
    },
    EnrollWorker {
        country: String,
        evidence: String,
        capacity: u32,
    },
    EnrollCluster {
        #[serde(rename = "workerID")]
        worker_id: String,
        namespace: String,
        country: String,
        evidence: String,
        capacity: u32,
    },
    EnableCluster {
        #[serde(rename = "clusterID")]
        cluster_id: String,
        enabled: bool,
    },
    RebindCluster {
        #[serde(rename = "clusterID")]
        cluster_id: String,
        #[serde(rename = "workerID")]
        worker_id: String,
    },
    ConfigureStorage {
        #[serde(rename = "clusterID")]
        cluster_id: String,
        policy: Option<crate::KubernetesStoragePolicy>,
        runtime_fingerprint: Option<String>,
    },
}
impl BuildService {
    pub async fn execute_build_infrastructure(
        &self,
        actor: &Principal,
        authority: &spitzli_browser_session::StoredAuthority,
        input: Value,
    ) -> Result<Value> {
        if serde_json::to_vec(&input)
            .map_err(|_| error("invalid_command"))?
            .len()
            > 8192
        {
            return Err(error("command_limit"));
        }
        let command: InfrastructureCommand =
            serde_json::from_value(input).map_err(|_| error("invalid_command"))?;
        let guard = self.begin_authority(actor, Some(authority), true).await?;
        // Each method separately checks current persisted MFA-enabled operator authority.
        let result = match command {
            InfrastructureCommand::Admission {
                enabled,
                global_slots,
                tenant_slots,
            } => {
                self.configure_kubernetes_admission(actor, enabled, global_slots, tenant_slots)
                    .await?;
                json!({"enabled":enabled,"publicReady":false})
            }
            InfrastructureCommand::TrustProject {
                customer_id,
                project_id,
                enabled,
            } => {
                self.trust_kubernetes_project(actor, &customer_id, &project_id, enabled)
                    .await?;
                json!({"projectID":project_id,"trusted":enabled})
            }
            InfrastructureCommand::EnrollWorker {
                country,
                evidence,
                capacity,
            } => {
                self.enroll_worker(actor, &country, "artifact-only", &evidence, capacity)
                    .await?
            }
            InfrastructureCommand::EnrollCluster {
                worker_id,
                namespace,
                country,
                evidence,
                capacity,
            } => {
                self.enroll_kubernetes_cluster(
                    actor, &worker_id, &namespace, &country, &evidence, capacity,
                )
                .await?
            }
            InfrastructureCommand::EnableCluster {
                cluster_id,
                enabled,
            } => {
                self.enable_kubernetes_cluster(actor, &cluster_id, enabled)
                    .await?;
                json!({"clusterID":cluster_id,"enabled":enabled})
            }
            InfrastructureCommand::RebindCluster {
                cluster_id,
                worker_id,
            } => {
                self.rebind_kubernetes_cluster(actor, &cluster_id, &worker_id)
                    .await?;
                json!({"clusterID":cluster_id,"enabled":false})
            }
            InfrastructureCommand::ConfigureStorage {
                cluster_id,
                policy,
                runtime_fingerprint,
            } => {
                self.configure_kubernetes_storage(
                    actor,
                    &cluster_id,
                    policy,
                    runtime_fingerprint.as_deref(),
                )
                .await?;
                json!({"clusterID":cluster_id,"configured":true,"publicReady":false})
            }
        };
        session::verify(&guard).await?;
        session::commit(guard).await?;
        Ok(result)
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn fixed_storage_configuration_is_bounded_nonsecret_and_cluster_scoped() {
        let value = serde_json::json!({"node":"webdock-build-eu-01","poolID":"123e4567-e89b-42d3-a456-426614174000","volumeBytes":28991029248_u64});
        let policy: crate::KubernetesStoragePolicy = serde_json::from_value(value.clone()).unwrap();
        policy.validate("78").unwrap();
        for (key, bad) in [
            ("node", serde_json::json!("../run-node")),
            ("poolID", serde_json::json!("unknown")),
            ("volumeBytes", serde_json::json!(67108864)),
        ] {
            let mut other = value.clone();
            other[key] = bad;
            let policy: crate::KubernetesStoragePolicy = serde_json::from_value(other).unwrap();
            assert!(policy.validate("78").is_err(), "{key}");
        }
        let mut credentials = value;
        credentials["kubeconfig"] = serde_json::json!("not accepted");
        assert!(serde_json::from_value::<crate::KubernetesStoragePolicy>(credentials).is_err());
    }
    use super::*;
    #[test]
    fn operator_inputs_cannot_inject_kubernetes_or_credentials() {
        for input in [
            json!({"action":"enrollCluster","workerID":"123","namespace":"webdock-build-qualification","country":"DE","evidence":"verified owned isolated build host","capacity":1,"kubeconfig":"secret"}),
            json!({"action":"enrollWorker","country":"DE","evidence":"verified owned isolated build host","capacity":1,"isolation":"microvm"}),
        ] {
            assert!(serde_json::from_value::<InfrastructureCommand>(input).is_err());
        }
    }
}
