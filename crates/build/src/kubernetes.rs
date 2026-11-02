//! Opt-in trusted-project Jobs on an explicitly injected build-cluster client.
//!
//! The operator must qualify the configured RuntimeClass as a sandbox and enforce
//! namespace network/admission policy. Transfer capabilities remain in immutable
//! per-attempt handoffs; completed Jobs are never registry-verified build results.
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use futures_util::io::AsyncReadExt;
use k8s_openapi::{
    ByteString,
    api::{
        batch::v1::Job,
        core::v1::{Pod, Secret},
    },
    apimachinery::pkg::apis::meta::v1::ObjectMeta,
};
use kube::{
    Api, Client,
    api::{
        ApiResource, DeleteParams, DynamicObject, ListParams, LogParams, Patch, PatchParams,
        PostParams, Preconditions,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const FINGERPRINT: &str = "webdock.dev/build-fingerprint";
const HANDOFF_DIGEST: &str = "webdock.dev/handoff-sha256";
const MAX_HANDOFF_BYTES: usize = 96 * 1024;
const MAX_API_SECONDS: u64 = 15;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedProject {
    pub customer_id: String,
    pub project_id: String,
}

/// Operator-owned configuration; never construct this from customer build inputs.
#[derive(Clone, Debug)]
pub struct KubernetesBuildConfig {
    pub profile: KubernetesBuildProfile,
    pub namespace: String,
    pub service_account: String,
    pub sandbox_runtime_class: String,
    pub runner_image: String,
    pub trusted_projects: BTreeSet<TrustedProject>,
    /// None preserves the trusted pilot. A cluster ID requires a bound fixed volume.
    pub storage_cluster_id: Option<String>,
    pub hold_execution: bool,
}

/// The id-map exception is qualified only for the fixed private pilot namespace
/// and pinned runsc systrap installation; it is never a customer Pod override.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum KubernetesBuildProfile {
    Restricted,
    RootlessIdmapSandbox,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KubernetesBuildLimits {
    pub cpu_millis: u32,
    pub memory_mib: u32,
    pub workspace_mib: u32,
    pub cache_mib: u32,
    pub deadline_seconds: u32,
}

/// Identity and bounded resources only: no scripts, URLs, secrets or Pod overrides.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KubernetesBuildRequest {
    pub customer_id: String,
    pub project_id: String,
    pub build_id: String,
    pub generation: u64,
    pub attempt: u32,
    pub commit_sha: String,
    pub limits: KubernetesBuildLimits,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<webdock_agent_protocol::build_storage::BuildVolumeHandle>,
}

/// Persist this after create/adoption. Every operation revalidates it and the live Job.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KubernetesJobHandle {
    pub request: KubernetesBuildRequest,
    pub namespace: String,
    pub uid: String,
}

/// Safe to persist: capabilities remain only in the immutable Secret.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KubernetesHandoffHandle {
    pub request: KubernetesBuildRequest,
    pub namespace: String,
    pub uid: String,
    pub sha256: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum KubernetesJobPhase {
    Pending,
    Running,
    Failed,
    Deleting,
    SucceededAwaitingRegistryVerification,
}

#[derive(Clone, Copy, Debug)]
pub struct KubernetesLogBounds {
    pub max_bytes: u32,
    pub tail_lines: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KubernetesBuildLogs {
    /// Raw bytes preserve the strict cap even when a container emits invalid UTF-8.
    pub bytes: Vec<u8>,
    pub truncated: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum KubernetesBuildError {
    #[error("invalid build-cluster configuration")]
    InvalidConfig,
    #[error("invalid Kubernetes build request")]
    InvalidRequest,
    #[error("project is not explicitly trusted for Kubernetes builds")]
    ProjectNotTrusted,
    #[error("Kubernetes Job identity or immutable specification changed")]
    IdentityConflict,
    #[error("build-cluster operation timed out; its outcome may be unknown")]
    Timeout,
    #[error(transparent)]
    Api(#[from] kube::Error),
    #[error("invalid Kubernetes manifest")]
    Manifest(#[from] serde_json::Error),
    #[error("build log stream failed")]
    Logs(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, KubernetesBuildError>;

fn classify_failure(
    job_status: &Value,
    pod: Option<&k8s_openapi::api::core::v1::PodStatus>,
) -> &'static str {
    if job_status["conditions"].as_array().is_some_and(|cs| {
        cs.iter().any(|c| {
            c["type"] == "Failed" && c["status"] == "True" && c["reason"] == "DeadlineExceeded"
        })
    }) {
        return "BUILD_DEADLINE_EXCEEDED";
    }
    if let Some(pod) = pod {
        if pod.reason.as_deref() == Some("NodeLost") {
            return "KUBERNETES_NODE_UNAVAILABLE";
        }
        if pod.reason.as_deref() == Some("Evicted") {
            return "BUILD_RESOURCE_EVICTED";
        }
        if pod
            .container_statuses
            .iter()
            .flatten()
            .chain(pod.init_container_statuses.iter().flatten())
            .any(|c| {
                c.state
                    .as_ref()
                    .and_then(|s| s.terminated.as_ref())
                    .and_then(|t| t.reason.as_deref())
                    == Some("OOMKilled")
            })
        {
            return "BUILD_MEMORY_LIMIT";
        }
    }
    "BUILD_FAILED"
}

#[cfg(test)]
mod failure_tests {
    use super::*;
    #[test]
    fn only_trusted_status_fields_classify_resource_and_node_failures() {
        let pod = |v| serde_json::from_value::<k8s_openapi::api::core::v1::PodStatus>(v).unwrap();
        assert_eq!(
            classify_failure(
                &json!({"conditions":[{"type":"Failed","status":"True","reason":"DeadlineExceeded"}]}),
                None
            ),
            "BUILD_DEADLINE_EXCEEDED"
        );
        assert_eq!(
            classify_failure(&Value::Null, Some(&pod(json!({"reason":"NodeLost"})))),
            "KUBERNETES_NODE_UNAVAILABLE"
        );
        assert_eq!(
            classify_failure(&Value::Null, Some(&pod(json!({"reason":"Evicted"})))),
            "BUILD_RESOURCE_EVICTED"
        );
        let oom = pod(
            json!({"containerStatuses":[{"name":"runner","image":"pinned","imageID":"pinned","ready":false,"restartCount":0,"state":{"terminated":{"exitCode":137,"reason":"OOMKilled"}}}]}),
        );
        assert_eq!(
            classify_failure(&Value::Null, Some(&oom)),
            "BUILD_MEMORY_LIMIT"
        );
        assert_eq!(
            classify_failure(
                &json!({"message":"NodeLost DeadlineExceeded"}),
                Some(&pod(
                    json!({"message":"OOMKilled KUBERNETES_NODE_UNAVAILABLE"})
                ))
            ),
            "BUILD_FAILED"
        );
    }
}

#[derive(Clone)]
pub struct KubernetesJobBuildExecutor {
    client: Client,
    config: KubernetesBuildConfig,
}

fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.as_bytes()[s.len() - 1].is_ascii_alphanumeric()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn dns_label(s: &str) -> bool {
    identifier(s)
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn lowercase_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn pinned_image(s: &str) -> bool {
    let Some((repository, digest)) = s.split_once("@sha256:") else {
        return false;
    };
    !repository.is_empty()
        && repository.len() <= 255
        && repository.as_bytes()[0].is_ascii_alphanumeric()
        && repository
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"/._-:".contains(&b))
        && !repository.contains("..")
        && lowercase_hex(digest, 64)
}

fn digest(value: &Value) -> String {
    hex::encode(Sha256::digest(value.to_string().as_bytes()))
}

fn mib_quantity(value: u32) -> String {
    if value.is_multiple_of(1024) {
        format!("{}Gi", value / 1024)
    } else {
        format!("{value}Mi")
    }
}

fn cpu_quantity(millis: u32) -> String {
    if millis.is_multiple_of(1000) {
        (millis / 1000).to_string()
    } else {
        format!("{millis}m")
    }
}

fn fixed_container_storage(containers: &mut Value) -> Result<()> {
    for container in containers
        .as_array_mut()
        .ok_or(KubernetesBuildError::InvalidRequest)?
    {
        for mount in container["volumeMounts"]
            .as_array_mut()
            .ok_or(KubernetesBuildError::InvalidRequest)?
        {
            if mount["name"] == "workspace" || mount["name"] == "cache" {
                mount["subPath"] = mount["name"].clone();
                mount["name"] = json!("build");
            }
        }
        for resources in ["requests", "limits"] {
            container["resources"][resources]["ephemeral-storage"] = json!("64Mi");
        }
    }
    Ok(())
}

impl KubernetesJobBuildExecutor {
    /// No `Client::try_default`, ambient kubeconfig, or default namespace fallback.
    pub fn new(client: Client, config: KubernetesBuildConfig) -> Result<Self> {
        if !dns_label(&config.namespace)
            || ["default", "kube-system", "kube-public", "kube-node-lease"]
                .contains(&config.namespace.as_str())
            || !dns_label(&config.service_account)
            || config.service_account == "default"
            || !dns_label(&config.sandbox_runtime_class)
            || ["default", "runc", "crun", "containerd"]
                .contains(&config.sandbox_runtime_class.as_str())
            || !pinned_image(&config.runner_image)
            || (config.profile == KubernetesBuildProfile::RootlessIdmapSandbox
                && (config.namespace != "webdock-build-qualification"
                    || config.sandbox_runtime_class != "webdock-build-sandbox"))
            || config.storage_cluster_id.as_ref().is_some_and(|id| {
                crate::contracts::id(id).is_err()
                    || config.profile != KubernetesBuildProfile::RootlessIdmapSandbox
            })
            || config
                .trusted_projects
                .iter()
                .any(|p| !identifier(&p.customer_id) || !identifier(&p.project_id))
        {
            return Err(KubernetesBuildError::InvalidConfig);
        }
        Ok(Self { client, config })
    }

    /// Stable execution-policy identity; allowlist membership is checked separately.
    pub fn runtime_fingerprint(&self) -> String {
        if self.config.hold_execution || self.config.storage_cluster_id.is_some() {
            return digest(
                &json!({"schema":4,"namespace":self.config.namespace,"profile":self.config.profile,
                "idmapQualification":"runsc-release-20261026.0-systrap-network-sandbox-allow-suid",
                "serviceAccount":self.config.service_account,"runtimeClass":self.config.sandbox_runtime_class,
                "runnerImage":self.config.runner_image,"storageProfile":self.config.storage_cluster_id.as_ref().map(|_|"fixed-ext4-v1"),"storageClusterID":self.config.storage_cluster_id,"holdExecution":self.config.hold_execution}),
            );
        }
        digest(
            &json!({"schema":3,"namespace":self.config.namespace,"profile":self.config.profile,
            "idmapQualification":"runsc-release-20261026.0-systrap-network-sandbox-allow-suid",
            "serviceAccount":self.config.service_account,"runtimeClass":self.config.sandbox_runtime_class,
            "runnerImage":self.config.runner_image}),
        )
    }

    fn validate_identity(&self, request: &KubernetesBuildRequest) -> Result<()> {
        let l = &request.limits;
        if !identifier(&request.customer_id)
            || !identifier(&request.project_id)
            || !identifier(&request.build_id)
            || request.generation == 0
            || request.generation != u64::from(request.attempt)
            || !(lowercase_hex(&request.commit_sha, 40) || lowercase_hex(&request.commit_sha, 64))
            || !(250..=8000).contains(&l.cpu_millis)
            || !(512..=16384).contains(&l.memory_mib)
            || !(512..=32768).contains(&l.workspace_mib)
            || !(64..=16384).contains(&l.cache_mib)
            || !(30..=1800).contains(&l.deadline_seconds)
        {
            return Err(KubernetesBuildError::InvalidRequest);
        }
        Ok(())
    }
    fn validate(&self, request: &KubernetesBuildRequest) -> Result<()> {
        self.validate_identity(request)?;
        let l = &request.limits;
        match (&self.config.storage_cluster_id, &request.volume) {
            (None, None) => {}
            (Some(cluster), Some(volume)) => {
                volume
                    .validate_for(&volume.reservation)
                    .map_err(|_| KubernetesBuildError::InvalidRequest)?;
                let r = &volume.reservation;
                let minimum = u64::from(l.workspace_mib + l.cache_mib) * 1024 * 1024;
                if r.customer_id != request.customer_id
                    || r.project_id != request.project_id
                    || r.build_id != request.build_id
                    || r.generation != request.generation
                    || &r.target.cluster_id != cluster
                    || volume.namespace != self.config.namespace
                    || r.volume_bytes < minimum
                    || r.volume_bytes > minimum + 4 * 1024 * 1024 * 1024
                {
                    return Err(KubernetesBuildError::InvalidRequest);
                }
            }
            _ => return Err(KubernetesBuildError::InvalidRequest),
        }
        Ok(())
    }

    fn admit(&self, request: &KubernetesBuildRequest) -> Result<()> {
        self.validate(request)?;
        if !self.config.trusted_projects.contains(&TrustedProject {
            customer_id: request.customer_id.clone(),
            project_id: request.project_id.clone(),
        }) {
            return Err(KubernetesBuildError::ProjectNotTrusted);
        }
        Ok(())
    }

    pub fn job_name(&self, request: &KubernetesBuildRequest) -> Result<String> {
        self.validate_identity(request)?;
        let hash = hex::encode(Sha256::digest(request.build_id.as_bytes()));
        Ok(format!("wd-build-{}-{}", &hash[..40], request.attempt))
    }

    pub fn fingerprint(&self, request: &KubernetesBuildRequest) -> Result<String> {
        self.manifest(request)?
            .metadata
            .annotations
            .and_then(|a| a.get(FINGERPRINT).cloned())
            .ok_or(KubernetesBuildError::IdentityConflict)
    }
    pub fn requires_storage(&self) -> bool {
        self.config.storage_cluster_id.is_some()
    }

    pub fn handoff_name(&self, request: &KubernetesBuildRequest) -> Result<String> {
        Ok(self
            .job_name(request)?
            .replacen("wd-build-", "wd-input-", 1))
    }

    pub fn manifest(&self, request: &KubernetesBuildRequest) -> Result<Job> {
        self.admit(request)?;
        self.bound_manifest(request)
    }

    fn bound_manifest(&self, request: &KubernetesBuildRequest) -> Result<Job> {
        let name = self.job_name(request)?;
        let l = &request.limits;
        let labels = json!({"webdock.dev/component":"build-job", "webdock.dev/customer-id":request.customer_id,
            "webdock.dev/project-id":request.project_id,"webdock.dev/build-id":request.build_id,
            "webdock.dev/generation":request.generation.to_string(),"webdock.dev/attempt":request.attempt.to_string()});
        let resources = json!({"cpu":cpu_quantity(l.cpu_millis),"memory":mib_quantity(l.memory_mib),
            "ephemeral-storage":mib_quantity(l.workspace_mib + l.cache_mib + 64)});
        let mut manifest = json!({"apiVersion":"batch/v1","kind":"Job",
            "metadata":{"name":name,"namespace":self.config.namespace,"labels":labels},
            "spec":{"parallelism":1,"completions":1,"backoffLimit":0,
                "activeDeadlineSeconds":l.deadline_seconds,
                "template":{"metadata":{"labels":labels},"spec":{
                    "runtimeClassName":self.config.sandbox_runtime_class,
                    "serviceAccountName":self.config.service_account,"automountServiceAccountToken":false,
                    "restartPolicy":"Never","terminationGracePeriodSeconds":10,"activeDeadlineSeconds":l.deadline_seconds,
                    "hostNetwork":false,"hostPID":false,"hostIPC":false,"enableServiceLinks":false,
                    "securityContext":{"runAsNonRoot":true,"runAsUser":10001,"runAsGroup":10001,
                        "fsGroup":10001,"seccompProfile":{"type":"RuntimeDefault"}},
                    "containers":[{"name":"runner","image":self.config.runner_image,"imagePullPolicy":"IfNotPresent",
                        "command":["/usr/local/bin/webdock-build-runner"],
                        "env":[{"name":"TMPDIR","value":"/workspace"},
                            {"name":"WEBDOCK_BUILD_ID","value":request.build_id},
                            {"name":"WEBDOCK_CUSTOMER_ID","value":request.customer_id},
                            {"name":"WEBDOCK_PROJECT_ID","value":request.project_id},
                            {"name":"WEBDOCK_GENERATION","value":request.generation.to_string()},
                            {"name":"WEBDOCK_COMMIT_SHA","value":request.commit_sha}],
                        "resources":{"requests":resources,"limits":resources},
                        "securityContext":{"privileged":false,"allowPrivilegeEscalation":false,
                            "readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}},
                        "volumeMounts":[{"name":"workspace","mountPath":"/workspace"},
                            {"name":"cache","mountPath":"/cache"},{"name":"tmp","mountPath":"/tmp"}]}],
                    "volumes":[{"name":"workspace","emptyDir":{"sizeLimit":mib_quantity(l.workspace_mib)}},
                        {"name":"cache","emptyDir":{"sizeLimit":mib_quantity(l.cache_mib)}},
                        {"name":"tmp","emptyDir":{"sizeLimit":"64Mi"}}]
                }}}
        });
        if self.config.profile == KubernetesBuildProfile::RootlessIdmapSandbox {
            manifest["metadata"]["labels"]["webdock.dev/profile"] = json!("rootless-idmap-sandbox");
            manifest["spec"]["template"]["metadata"]["labels"]["webdock.dev/profile"] =
                json!("rootless-idmap-sandbox");
            let pod = &mut manifest["spec"]["template"]["spec"];
            pod["containers"][0]["command"] = json!(["/usr/local/bin/webdock-build-entrypoint"]);
            for key in ["runAsUser", "runAsGroup", "fsGroup"] {
                pod["securityContext"][key] = json!(1000);
            }
            pod["containers"][0]["securityContext"]["allowPrivilegeEscalation"] = json!(true);
            pod["containers"][0]["securityContext"]["capabilities"]["add"] =
                json!(["SETUID", "SETGID"]);
            let handoff = json!({"name":"handoff","mountPath":"/handoff/attempt.json","subPath":"attempt.json","readOnly":true});
            pod["containers"][0]["volumeMounts"]
                .as_array_mut()
                .ok_or(KubernetesBuildError::InvalidConfig)?
                .push(handoff.clone());
            pod["containers"][0]["env"].as_array_mut().ok_or(KubernetesBuildError::InvalidConfig)?.push(json!({"name":"WEBDOCK_JOB_UID","valueFrom":{"fieldRef":{"apiVersion":"v1","fieldPath":"metadata.labels['batch.kubernetes.io/controller-uid']"}}}));
            pod["volumes"].as_array_mut().ok_or(KubernetesBuildError::InvalidConfig)?.push(json!({"name":"handoff","secret":{"secretName":self.handoff_name(request)?,"defaultMode":288,"items":[{"key":"attempt.json","path":"attempt.json"}]}}));
            pod["initContainers"] = json!([{"name":"download","image":self.config.runner_image,"imagePullPolicy":"IfNotPresent",
                "command":["/usr/local/bin/webdock-build-runner"],"args":["download","--config","/handoff/attempt.json","--policy","/etc/webdock-build/transfer-policy.json","--input-dir","/workspace/input"],
                "env":[{"name":"TMPDIR","value":"/workspace"}],"resources":{"requests":resources,"limits":resources},
                "securityContext":{"privileged":false,"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}},
                "volumeMounts":[{"name":"workspace","mountPath":"/workspace"},{"name":"tmp","mountPath":"/tmp"},handoff]}]);
        }
        if let Some(volume) = &request.volume {
            let allocation = volume
                .reservation
                .allocation_fingerprint()
                .map_err(|_| KubernetesBuildError::InvalidRequest)?;
            for path in ["/metadata/labels", "/spec/template/metadata/labels"] {
                let labels = manifest
                    .pointer_mut(path)
                    .ok_or(KubernetesBuildError::InvalidRequest)?;
                labels["webdock.dev/volume-id"] = json!(volume.reservation.volume_id);
            }
            manifest["metadata"]["annotations"]["webdock.dev/allocation-sha256"] =
                json!(allocation);
            manifest["spec"]["template"]["metadata"]["annotations"]["webdock.dev/allocation-sha256"] =
                json!(allocation);
            let pod = &mut manifest["spec"]["template"]["spec"];
            // All disk writes are on one fixed filesystem; scratch consumes only
            // bounded memory. No dynamic provisioner or host path enters a Job.
            let handoff = pod["volumes"]
                .as_array()
                .ok_or(KubernetesBuildError::InvalidRequest)?
                .iter()
                .find(|v| v["name"] == "handoff")
                .cloned()
                .ok_or(KubernetesBuildError::InvalidRequest)?;
            pod["volumes"] = json!([{"name":"build","persistentVolumeClaim":{"claimName":volume.pvc_name,"readOnly":false}},
                {"name":"tmp","emptyDir":{"medium":"Memory","sizeLimit":"64Mi"}},handoff]);
            for containers in ["containers", "initContainers"] {
                fixed_container_storage(&mut pod[containers])?;
            }
            // Actual resource UIDs are part of immutable execution identity.
            let fp = volume
                .fingerprint()
                .map_err(|_| KubernetesBuildError::InvalidRequest)?;
            manifest["metadata"]["annotations"]["webdock.dev/volume-fingerprint"] = json!(fp);
            manifest["spec"]["template"]["metadata"]["annotations"]["webdock.dev/volume-fingerprint"] =
                json!(fp);
        }
        if self.config.hold_execution {
            manifest["spec"]["suspend"] = json!(true);
        }
        let fingerprint = digest(&manifest);
        manifest["metadata"]["annotations"][FINGERPRINT] = json!(fingerprint);
        manifest["spec"]["template"]["metadata"]["annotations"][FINGERPRINT] = json!(fingerprint);
        Ok(serde_json::from_value(manifest)?)
    }

    fn jobs(&self) -> Api<DynamicObject> {
        // Preserve fields newer than the compiled OpenAPI schema so they cannot
        // disappear before immutable-spec verification.
        Api::namespaced_with(
            self.client.clone(),
            &self.config.namespace,
            &ApiResource::erase::<Job>(&()),
        )
    }

    pub fn namespace(&self) -> &str {
        &self.config.namespace
    }

    /// Read-only API/role reachability before fresh admission; not proof of runtime or node readiness.
    pub async fn available(&self) -> Result<()> {
        tokio::time::timeout(
            Duration::from_secs(MAX_API_SECONDS),
            self.jobs().list(&ListParams::default().limit(1)),
        )
        .await
        .map_err(|_| KubernetesBuildError::Timeout)??;
        Ok(())
    }

    fn secrets(&self) -> Api<Secret> {
        Api::namespaced(self.client.clone(), &self.config.namespace)
    }

    fn handoff_profile(&self, request: &KubernetesBuildRequest) -> Result<()> {
        self.validate(request)?;
        if self.config.profile != KubernetesBuildProfile::RootlessIdmapSandbox {
            return Err(KubernetesBuildError::InvalidConfig);
        }
        Ok(())
    }

    fn verify_handoff(
        &self,
        request: &KubernetesBuildRequest,
        sha256: &str,
        secret: &Secret,
    ) -> Result<()> {
        self.handoff_profile(request)?;
        let expected = self.bound_manifest(request)?;
        let data = secret
            .data
            .as_ref()
            .and_then(|data| {
                if data.len() == 1 {
                    data.get("attempt.json")
                } else {
                    None
                }
            })
            .ok_or(KubernetesBuildError::IdentityConflict)?;
        if secret.metadata.name.as_deref() != Some(self.handoff_name(request)?.as_str())
            || secret.metadata.namespace.as_deref() != Some(&self.config.namespace)
            || secret
                .metadata
                .uid
                .as_deref()
                .is_none_or(|uid| !identifier(uid))
            || secret.immutable != Some(true)
            || secret.type_.as_deref() != Some("Opaque")
            || data.0.len() > MAX_HANDOFF_BYTES
            || !lowercase_hex(sha256, 64)
            || hex::encode(Sha256::digest(&data.0)) != sha256
            || secret
                .metadata
                .annotations
                .as_ref()
                .and_then(|a| a.get(HANDOFF_DIGEST))
                .map(String::as_str)
                != Some(sha256)
            || secret
                .metadata
                .annotations
                .as_ref()
                .and_then(|a| a.get(FINGERPRINT))
                != expected
                    .metadata
                    .annotations
                    .as_ref()
                    .and_then(|a| a.get(FINGERPRINT))
            || expected.metadata.labels.as_ref().is_none_or(|labels| {
                labels
                    .iter()
                    .any(|(k, v)| secret.metadata.labels.as_ref().and_then(|a| a.get(k)) != Some(v))
            })
        {
            return Err(KubernetesBuildError::IdentityConflict);
        }
        Ok(())
    }

    async fn get_handoff(&self, request: &KubernetesBuildRequest) -> Result<Option<Secret>> {
        match self.secrets().get(&self.handoff_name(request)?).await {
            Ok(secret) => Ok(Some(secret)),
            Err(kube::Error::Api(response)) if response.code == 404 => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Called only by the trusted staging backend. Identical retries may adopt an
    /// immutable handoff; different presigned capabilities never replace it.
    pub async fn stage_handoff(
        &self,
        request: &KubernetesBuildRequest,
        bytes: &[u8],
    ) -> Result<KubernetesHandoffHandle> {
        self.admit(request)?;
        self.handoff_profile(request)?;
        if bytes.is_empty()
            || bytes.len() > MAX_HANDOFF_BYTES
            || !serde_json::from_slice::<Value>(bytes).is_ok_and(|v| v.is_object())
        {
            return Err(KubernetesBuildError::InvalidRequest);
        }
        let sha256 = hex::encode(Sha256::digest(bytes));
        let job = self.bound_manifest(request)?;
        let mut annotations = job.metadata.annotations.unwrap_or_default();
        annotations.insert(HANDOFF_DIGEST.into(), sha256.clone());
        let secret = Secret {
            metadata: ObjectMeta {
                name: Some(self.handoff_name(request)?),
                namespace: Some(self.config.namespace.clone()),
                labels: job.metadata.labels,
                annotations: Some(annotations),
                ..ObjectMeta::default()
            },
            data: Some(BTreeMap::from([(
                "attempt.json".into(),
                ByteString(bytes.to_vec()),
            )])),
            immutable: Some(true),
            type_: Some("Opaque".into()),
            ..Secret::default()
        };
        let operation = async {
            let stored = match self.secrets().create(&PostParams::default(), &secret).await {
                Ok(stored) => stored,
                Err(kube::Error::Api(response)) if response.code == 409 => {
                    self.secrets().get(&self.handoff_name(request)?).await?
                }
                Err(error) => return Err(error.into()),
            };
            self.verify_handoff(request, &sha256, &stored)?;
            if stored.metadata.deletion_timestamp.is_some() {
                return Err(KubernetesBuildError::IdentityConflict);
            }
            Ok(KubernetesHandoffHandle {
                request: request.clone(),
                namespace: self.config.namespace.clone(),
                uid: stored
                    .metadata
                    .uid
                    .ok_or(KubernetesBuildError::IdentityConflict)?,
                sha256,
            })
        };
        tokio::time::timeout(Duration::from_secs(MAX_API_SECONDS), operation)
            .await
            .map_err(|_| KubernetesBuildError::Timeout)?
    }

    /// Read-only reconciliation after an uncertain staging response; never create.
    pub async fn observe_handoff(
        &self,
        request: &KubernetesBuildRequest,
        sha256: &str,
    ) -> Result<Option<KubernetesHandoffHandle>> {
        self.handoff_profile(request)?;
        if !lowercase_hex(sha256, 64) {
            return Err(KubernetesBuildError::InvalidRequest);
        }
        let operation = async {
            let Some(secret) = self.get_handoff(request).await? else {
                return Ok(None);
            };
            self.verify_handoff(request, sha256, &secret)?;
            Ok(Some(KubernetesHandoffHandle {
                request: request.clone(),
                namespace: self.config.namespace.clone(),
                uid: secret
                    .metadata
                    .uid
                    .ok_or(KubernetesBuildError::IdentityConflict)?,
                sha256: sha256.into(),
            }))
        };
        tokio::time::timeout(Duration::from_secs(MAX_API_SECONDS), operation)
            .await
            .map_err(|_| KubernetesBuildError::Timeout)?
    }

    /// Call after Job/Pod absence. Revocation does not prevent exact-UID cleanup.
    pub async fn delete_handoff(&self, handle: &KubernetesHandoffHandle) -> Result<Option<()>> {
        self.handoff_profile(&handle.request)?;
        if handle.namespace != self.config.namespace
            || !identifier(&handle.uid)
            || !lowercase_hex(&handle.sha256, 64)
        {
            return Err(KubernetesBuildError::IdentityConflict);
        }
        let operation = async {
            let Some(secret) = self.get_handoff(&handle.request).await? else {
                return Ok(None);
            };
            self.verify_handoff(&handle.request, &handle.sha256, &secret)?;
            if secret.metadata.uid.as_deref() != Some(handle.uid.as_str()) {
                return Err(KubernetesBuildError::IdentityConflict);
            }
            let params = DeleteParams {
                preconditions: Some(Preconditions {
                    uid: Some(handle.uid.clone()),
                    resource_version: Some(
                        secret
                            .metadata
                            .resource_version
                            .ok_or(KubernetesBuildError::IdentityConflict)?,
                    ),
                }),
                ..DeleteParams::foreground()
            };
            match self
                .secrets()
                .delete(&self.handoff_name(&handle.request)?, &params)
                .await
            {
                Ok(_) => Ok(Some(())),
                Err(kube::Error::Api(response)) if response.code == 404 => Ok(None),
                Err(error) => Err(error.into()),
            }
        };
        tokio::time::timeout(Duration::from_secs(MAX_API_SECONDS), operation)
            .await
            .map_err(|_| KubernetesBuildError::Timeout)?
    }
    fn pods(&self) -> Api<Pod> {
        Api::namespaced(self.client.clone(), &self.config.namespace)
    }

    fn verify_job(&self, request: &KubernetesBuildRequest, job: &DynamicObject) -> Result<()> {
        let expected = self.bound_manifest(request)?;
        let uid = job
            .metadata
            .uid
            .as_deref()
            .filter(|u| identifier(u))
            .ok_or(KubernetesBuildError::IdentityConflict)?;
        if job.metadata.name != expected.metadata.name
            || job.metadata.namespace != expected.metadata.namespace
            || job
                .metadata
                .annotations
                .as_ref()
                .and_then(|a| a.get(FINGERPRINT))
                != expected
                    .metadata
                    .annotations
                    .as_ref()
                    .and_then(|a| a.get(FINGERPRINT))
            || expected.metadata.labels.as_ref().is_none_or(|labels| {
                labels.iter().any(|(key, value)| {
                    job.metadata.labels.as_ref().and_then(|l| l.get(key)) != Some(value)
                })
            })
        {
            return Err(KubernetesBuildError::IdentityConflict);
        }
        let mut spec = job
            .data
            .get("spec")
            .cloned()
            .ok_or(KubernetesBuildError::IdentityConflict)?;
        let mut expected_spec = serde_json::to_value(&expected.spec)?;
        remove_default(&mut spec, "manualSelector", json!(false));
        remove_default(&mut spec, "completionMode", json!("NonIndexed"));
        remove_default(
            &mut spec,
            "podReplacementPolicy",
            json!("TerminatingOrFailed"),
        );
        if self.config.hold_execution {
            if spec["suspend"].as_bool().is_none() {
                return Err(KubernetesBuildError::IdentityConflict);
            }
            spec.as_object_mut()
                .ok_or(KubernetesBuildError::IdentityConflict)?
                .remove("suspend");
            expected_spec
                .as_object_mut()
                .ok_or(KubernetesBuildError::IdentityConflict)?
                .remove("suspend");
        } else {
            remove_default(&mut spec, "suspend", json!(false));
        }
        if let Some(selector) = spec.get("selector") {
            let labels = selector
                .get("matchLabels")
                .and_then(Value::as_object)
                .ok_or(KubernetesBuildError::IdentityConflict)?;
            if selector.as_object().is_none_or(|s| s.len() != 1)
                || labels.is_empty()
                || labels.iter().any(|(k, v)| {
                    !["controller-uid", "batch.kubernetes.io/controller-uid"].contains(&k.as_str())
                        || v != uid
                })
            {
                return Err(KubernetesBuildError::IdentityConflict);
            }
            spec.as_object_mut()
                .ok_or(KubernetesBuildError::IdentityConflict)?
                .remove("selector");
        }
        if let Some(labels) = spec
            .pointer_mut("/template/metadata/labels")
            .and_then(Value::as_object_mut)
        {
            for (key, value) in [
                ("controller-uid", uid),
                ("batch.kubernetes.io/controller-uid", uid),
                ("job-name", self.job_name(request)?.as_str()),
                (
                    "batch.kubernetes.io/job-name",
                    self.job_name(request)?.as_str(),
                ),
            ] {
                if labels.get(key).is_some_and(|v| v == value) {
                    labels.remove(key);
                }
            }
        }
        remove_default(
            &mut spec["template"]["metadata"],
            "creationTimestamp",
            Value::Null,
        );
        normalize_pod_spec(&mut spec["template"]["spec"], &self.config.service_account);
        normalize_pod_spec(
            &mut expected_spec["template"]["spec"],
            &self.config.service_account,
        );
        if spec != expected_spec {
            return Err(KubernetesBuildError::IdentityConflict);
        }
        Ok(())
    }

    /// Create once; an API conflict is adopted only after verifying the entire immutable spec.
    /// A timeout is deliberately not retried here: reconcile the durable attempt first.
    pub async fn start(&self, request: &KubernetesBuildRequest) -> Result<KubernetesJobHandle> {
        let manifest: DynamicObject =
            serde_json::from_value(serde_json::to_value(self.manifest(request)?)?)?;
        let name = self.job_name(request)?;
        let jobs = self.jobs();
        let operation = async {
            self.verify_bound_claim(request).await?;
            let job = match jobs.create(&PostParams::default(), &manifest).await {
                Ok(job) => job,
                Err(kube::Error::Api(response)) if response.code == 409 => jobs.get(&name).await?,
                Err(error) => return Err(error.into()),
            };
            self.verify_job(request, &job)?;
            if job.metadata.deletion_timestamp.is_some() {
                return Err(KubernetesBuildError::IdentityConflict);
            }
            Ok(KubernetesJobHandle {
                request: request.clone(),
                namespace: self.config.namespace.clone(),
                uid: job
                    .metadata
                    .uid
                    .ok_or(KubernetesBuildError::IdentityConflict)?,
            })
        };
        tokio::time::timeout(Duration::from_secs(MAX_API_SECONDS), operation)
            .await
            .map_err(|_| KubernetesBuildError::Timeout)?
    }

    async fn verify_bound_claim(&self, request: &KubernetesBuildRequest) -> Result<()> {
        let Some(volume) = &request.volume else {
            return Ok(());
        };
        let resource = ApiResource::from_gvk(&kube::api::GroupVersionKind::gvk(
            "",
            "v1",
            "PersistentVolumeClaim",
        ));
        let claims: Api<DynamicObject> =
            Api::namespaced_with(self.client.clone(), &self.config.namespace, &resource);
        let claim = claims.get(&volume.pvc_name).await?;
        let claim = serde_json::to_value(claim)?;
        let r = &volume.reservation;
        let allocation = r
            .allocation_fingerprint()
            .map_err(|_| KubernetesBuildError::IdentityConflict)?;
        let generation = r.generation.to_string();
        if claim["metadata"]["name"] != volume.pvc_name
            || claim["metadata"]["namespace"] != volume.namespace
            || claim["metadata"]["uid"] != volume.pvc_uid
            || !claim["metadata"]["deletionTimestamp"].is_null()
            || claim["status"]["phase"] != "Bound"
            || claim["spec"]["volumeName"] != volume.pv_name
            || claim["spec"]["storageClassName"] != ""
            || claim["spec"]["volumeMode"] != "Filesystem"
            || claim["spec"]["accessModes"] != json!(["ReadWriteOnce"])
            || claim["metadata"]["annotations"]["webdock.dev/allocation-sha256"] != allocation
            || [
                ("webdock.dev/customer-id", r.customer_id.as_str()),
                ("webdock.dev/project-id", r.project_id.as_str()),
                ("webdock.dev/build-id", r.build_id.as_str()),
                ("webdock.dev/generation", generation.as_str()),
                ("webdock.dev/volume-id", r.volume_id.as_str()),
                ("app.kubernetes.io/managed-by", "webdock-build-storage"),
            ]
            .into_iter()
            .any(|(k, v)| claim["metadata"]["labels"][k] != v)
        {
            return Err(KubernetesBuildError::IdentityConflict);
        }
        let storage = claim["spec"]["resources"]["requests"]["storage"]
            .as_str()
            .ok_or(KubernetesBuildError::IdentityConflict)?;
        // Node requests use whole bytes; accept the API's equivalent binary units.
        let bytes = r.volume_bytes;
        if storage != bytes.to_string()
            && ![
                (1024, "Ki"),
                (1024 * 1024, "Mi"),
                (1024 * 1024 * 1024, "Gi"),
            ]
            .into_iter()
            .any(|(factor, suffix)| {
                bytes.is_multiple_of(factor) && storage == format!("{}{suffix}", bytes / factor)
            })
        {
            return Err(KubernetesBuildError::IdentityConflict);
        }
        Ok(())
    }

    fn validate_handle(&self, handle: &KubernetesJobHandle) -> Result<()> {
        self.validate(&handle.request)?;
        if handle.namespace != self.config.namespace || !identifier(&handle.uid) {
            return Err(KubernetesBuildError::IdentityConflict);
        }
        Ok(())
    }
    pub async fn release(&self, handle: &KubernetesJobHandle) -> Result<()> {
        self.validate_handle(handle)?;
        if !self.config.hold_execution {
            return Ok(());
        }
        let operation = async {
            let job = self
                .get_owned(handle)
                .await?
                .ok_or(KubernetesBuildError::IdentityConflict)?;
            if job.metadata.deletion_timestamp.is_some() {
                return Err(KubernetesBuildError::IdentityConflict);
            }
            if job.data["spec"]["suspend"] == false {
                return Ok(());
            }
            let rv = job
                .metadata
                .resource_version
                .ok_or(KubernetesBuildError::IdentityConflict)?;
            let patch = json!({"metadata":{"uid":handle.uid,"resourceVersion":rv},"spec":{"suspend":false}});
            self.jobs()
                .patch(
                    &self.job_name(&handle.request)?,
                    &PatchParams::default(),
                    &Patch::Merge(patch),
                )
                .await?;
            Ok(())
        };
        tokio::time::timeout(Duration::from_secs(MAX_API_SECONDS), operation)
            .await
            .map_err(|_| KubernetesBuildError::Timeout)?
    }

    async fn get_job(&self, name: &str) -> Result<Option<DynamicObject>> {
        match self.jobs().get(name).await {
            Ok(job) => Ok(Some(job)),
            // `get_opt` also accepts a NotFound reason paired with an error code.
            // Only an explicit 404 may release a durable attempt's resources.
            Err(kube::Error::Api(response)) if response.code == 404 => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Reconcile a persisted create intent without issuing another create. Trust
    /// revocation prevents starts but must not prevent exact-identity cleanup.
    pub async fn observe(
        &self,
        request: &KubernetesBuildRequest,
        fingerprint: &str,
    ) -> Result<Option<KubernetesJobHandle>> {
        let expected = self.bound_manifest(request)?;
        if expected
            .metadata
            .annotations
            .as_ref()
            .and_then(|a| a.get(FINGERPRINT))
            .map(String::as_str)
            != Some(fingerprint)
        {
            return Err(KubernetesBuildError::IdentityConflict);
        }
        let operation = async {
            let Some(job) = self.get_job(&self.job_name(request)?).await? else {
                return Ok(None);
            };
            self.verify_job(request, &job)?;
            Ok(Some(KubernetesJobHandle {
                request: request.clone(),
                namespace: self.config.namespace.clone(),
                uid: job
                    .metadata
                    .uid
                    .ok_or(KubernetesBuildError::IdentityConflict)?,
            }))
        };
        tokio::time::timeout(Duration::from_secs(MAX_API_SECONDS), operation)
            .await
            .map_err(|_| KubernetesBuildError::Timeout)?
    }

    async fn get_owned(&self, handle: &KubernetesJobHandle) -> Result<Option<DynamicObject>> {
        self.validate_handle(handle)?;
        let job = self.get_job(&self.job_name(&handle.request)?).await?;
        if let Some(job) = &job {
            self.verify_job(&handle.request, job)?;
            if job.metadata.uid.as_deref() != Some(&handle.uid) {
                return Err(KubernetesBuildError::IdentityConflict);
            }
        }
        Ok(job)
    }

    pub async fn inspect(
        &self,
        handle: &KubernetesJobHandle,
    ) -> Result<Option<KubernetesJobPhase>> {
        let job =
            tokio::time::timeout(Duration::from_secs(MAX_API_SECONDS), self.get_owned(handle))
                .await
                .map_err(|_| KubernetesBuildError::Timeout)??;
        Ok(job.map(|job| {
            if job.metadata.deletion_timestamp.is_some() {
                return KubernetesJobPhase::Deleting;
            }
            let status = &job.data["status"];
            let condition = |name| {
                status["conditions"].as_array().is_some_and(|conditions| {
                    conditions
                        .iter()
                        .any(|c| c["type"] == name && c["status"] == "True")
                })
            };
            if condition("Failed") {
                KubernetesJobPhase::Failed
            } else if condition("Complete") && status["succeeded"].as_u64() == Some(1) {
                KubernetesJobPhase::SucceededAwaitingRegistryVerification
            } else if status["active"].as_u64().is_some_and(|active| active > 0) {
                KubernetesJobPhase::Running
            } else {
                KubernetesJobPhase::Pending
            }
        }))
    }

    /// Classify API-owned status fields only; customer logs/messages never authorize a retry.
    pub async fn failure_code(&self, handle: &KubernetesJobHandle) -> Result<&'static str> {
        let operation = async {
            let job = self
                .get_owned(handle)
                .await?
                .ok_or(KubernetesBuildError::IdentityConflict)?;
            let list = self
                .pods()
                .list(
                    &ListParams::default()
                        .labels(&format!(
                            "batch.kubernetes.io/controller-uid={}",
                            handle.uid
                        ))
                        .limit(2),
                )
                .await?;
            if list.items.len() > 1
                || list
                    .metadata
                    .continue_
                    .as_deref()
                    .is_some_and(|c| !c.is_empty())
            {
                return Err(KubernetesBuildError::IdentityConflict);
            }
            for pod in &list.items {
                self.verify_pod(handle, pod)?;
            }
            Ok(classify_failure(
                &job.data["status"],
                list.items.first().and_then(|p| p.status.as_ref()),
            ))
        };
        tokio::time::timeout(Duration::from_secs(MAX_API_SECONDS), operation)
            .await
            .map_err(|_| KubernetesBuildError::Timeout)?
    }

    /// `None` means the exact Job was already absent, not that its Pods are gone.
    pub async fn cancel(&self, handle: &KubernetesJobHandle) -> Result<Option<()>> {
        let operation = async {
            let Some(job) = self.get_owned(handle).await? else {
                return Ok(None);
            };
            let resource_version = job
                .metadata
                .resource_version
                .ok_or(KubernetesBuildError::IdentityConflict)?;
            let params = DeleteParams {
                preconditions: Some(Preconditions {
                    uid: Some(handle.uid.clone()),
                    resource_version: Some(resource_version),
                }),
                ..DeleteParams::foreground()
            };
            match self
                .jobs()
                .delete(&self.job_name(&handle.request)?, &params)
                .await
            {
                Ok(_) => Ok(Some(())),
                Err(kube::Error::Api(response)) if response.code == 404 => Ok(None),
                Err(error) => Err(error.into()),
            }
        };
        tokio::time::timeout(Duration::from_secs(MAX_API_SECONDS), operation)
            .await
            .map_err(|_| KubernetesBuildError::Timeout)?
    }

    /// Call only after the durable attempt has been fenced against further starts.
    /// A missing Job alone never proves its Pods have been garbage-collected.
    pub async fn absent(
        &self,
        request: &KubernetesBuildRequest,
        expected_uid: Option<&str>,
    ) -> Result<bool> {
        self.validate_identity(request)?;
        if self.requires_storage() && request.volume.is_none() && expected_uid.is_some() {
            return Err(KubernetesBuildError::IdentityConflict);
        }
        if expected_uid.is_some_and(|uid| !identifier(uid)) {
            return Err(KubernetesBuildError::IdentityConflict);
        }
        let operation = async {
            let name = self.job_name(request)?;
            if let Some(job) = self.get_job(&name).await? {
                self.verify_job(request, &job)?;
                if expected_uid.is_some_and(|uid| job.metadata.uid.as_deref() != Some(uid)) {
                    return Err(KubernetesBuildError::IdentityConflict);
                }
                return Ok(false);
            }
            // Any remaining Pod under this attempt name blocks retirement, even a
            // spoofed label or a Pod from an unexpectedly replaced Job.
            let pods = self
                .pods()
                .list(
                    &ListParams::default()
                        .labels(&format!("batch.kubernetes.io/job-name={name}"))
                        .limit(1),
                )
                .await?;
            Ok(pods.items.is_empty()
                && pods.metadata.continue_.as_deref().is_none_or(str::is_empty))
        };
        tokio::time::timeout(Duration::from_secs(MAX_API_SECONDS), operation)
            .await
            .map_err(|_| KubernetesBuildError::Timeout)?
    }

    fn verify_pod(&self, handle: &KubernetesJobHandle, pod: &Pod) -> Result<()> {
        let expected = self.bound_manifest(&handle.request)?;
        let name = self.job_name(&handle.request)?;
        if pod.metadata.namespace.as_deref() != Some(&self.config.namespace)
            || pod.metadata.name.as_deref().is_none_or(|n| !dns_label(n))
            || pod
                .metadata
                .uid
                .as_deref()
                .is_none_or(|uid| !identifier(uid))
            || pod.metadata.owner_references.as_ref().is_none_or(|owners| {
                owners.len() != 1
                    || owners.iter().any(|owner| {
                        owner.uid != handle.uid
                            || owner.name != name
                            || owner.kind != "Job"
                            || owner.api_version != "batch/v1"
                            || owner.controller != Some(true)
                    })
            })
            || pod
                .metadata
                .annotations
                .as_ref()
                .and_then(|a| a.get(FINGERPRINT))
                != expected
                    .metadata
                    .annotations
                    .as_ref()
                    .and_then(|a| a.get(FINGERPRINT))
            || expected.metadata.labels.as_ref().is_none_or(|labels| {
                labels
                    .iter()
                    .any(|(k, v)| pod.metadata.labels.as_ref().and_then(|l| l.get(k)) != Some(v))
            })
        {
            return Err(KubernetesBuildError::IdentityConflict);
        }
        Ok(())
    }

    /// Reads one owned runner's finite log tail, with an independent client byte cap.
    /// Rechecking the Pod UID after reading closes the Pod-name replacement race.
    pub async fn logs(
        &self,
        handle: &KubernetesJobHandle,
        bounds: KubernetesLogBounds,
    ) -> Result<Option<KubernetesBuildLogs>> {
        self.validate_handle(handle)?;
        if !(1..=262144).contains(&bounds.max_bytes) || !(1..=10000).contains(&bounds.tail_lines) {
            return Err(KubernetesBuildError::InvalidRequest);
        }
        let operation = async {
            if self.get_owned(handle).await?.is_none() {
                return Ok(None);
            }
            let pods = self.pods();
            let list = pods
                .list(
                    &ListParams::default()
                        .labels(&format!(
                            "batch.kubernetes.io/controller-uid={}",
                            handle.uid
                        ))
                        .limit(2),
                )
                .await?;
            if list.items.len() > 1
                || list
                    .metadata
                    .continue_
                    .as_deref()
                    .is_some_and(|c| !c.is_empty())
            {
                return Err(KubernetesBuildError::IdentityConflict);
            }
            let Some(pod) = list.items.first() else {
                return Ok(None);
            };
            self.verify_pod(handle, pod)?;
            // Init/download failures can leave the runner permanently waiting.
            // There are no runner logs yet; do not prevent Job failure cleanup
            // by requesting an unavailable container log subresource.
            if pod.status.as_ref().is_some_and(|status| {
                status.phase.as_deref() == Some("Pending")
                    || status
                        .container_statuses
                        .as_ref()
                        .is_some_and(|containers| {
                            containers.iter().any(|container| {
                                container.name == "runner"
                                    && container
                                        .state
                                        .as_ref()
                                        .is_some_and(|state| state.waiting.is_some())
                            })
                        })
            }) {
                return Ok(None);
            }
            let name = pod
                .metadata
                .name
                .as_deref()
                .ok_or(KubernetesBuildError::IdentityConflict)?;
            let params = LogParams {
                timestamps: true,
                container: Some("runner".into()),
                limit_bytes: Some(i64::from(bounds.max_bytes)),
                tail_lines: Some(i64::from(bounds.tail_lines)),
                since_seconds: Some(1800),
                follow: false,
                ..LogParams::default()
            };
            let stream = pods.log_stream(name, &params).await?;
            let mut bytes = Vec::new();
            stream
                .take(u64::from(bounds.max_bytes) + 1)
                .read_to_end(&mut bytes)
                .await?;
            let current = pods.get(name).await?;
            self.verify_pod(handle, &current)?;
            if current.metadata.uid != pod.metadata.uid {
                return Err(KubernetesBuildError::IdentityConflict);
            }
            let truncated = bytes.len() > bounds.max_bytes as usize;
            bytes.truncate(bounds.max_bytes as usize);
            Ok(Some(KubernetesBuildLogs { bytes, truncated }))
        };
        tokio::time::timeout(Duration::from_secs(MAX_API_SECONDS), operation)
            .await
            .map_err(|_| KubernetesBuildError::Timeout)?
    }
}

fn remove_default(value: &mut Value, key: &str, default: Value) {
    if value.get(key) == Some(&default)
        && let Some(object) = value.as_object_mut()
    {
        object.remove(key);
    }
}

// Only Kubernetes' harmless defaults are normalized; extra containers, credentials,
// mounts, security settings and admission-injected execution changes fail closed.
fn normalize_pod_spec(spec: &mut Value, service_account: &str) {
    if let Some(volumes) = spec.get_mut("volumes").and_then(Value::as_array_mut) {
        for volume in volumes {
            if let Some(claim) = volume.get_mut("persistentVolumeClaim") {
                remove_default(claim, "readOnly", json!(false));
            }
        }
    }
    for key in ["hostNetwork", "hostPID", "hostIPC"] {
        remove_default(spec, key, json!(false));
    }
    for (key, value) in [
        ("dnsPolicy", json!("ClusterFirst")),
        ("schedulerName", json!("default-scheduler")),
        ("preemptionPolicy", json!("PreemptLowerPriority")),
        ("priority", json!(0)),
        ("serviceAccount", json!(service_account)),
    ] {
        remove_default(spec, key, value);
    }
    for key in ["containers", "initContainers"] {
        if let Some(containers) = spec.get_mut(key).and_then(Value::as_array_mut) {
            for container in containers {
                remove_default(
                    container,
                    "terminationMessagePath",
                    json!("/dev/termination-log"),
                );
                remove_default(container, "terminationMessagePolicy", json!("File"));
            }
        }
    }
}
