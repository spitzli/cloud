use crate::{ExecutionError, Result};
use serde::{Deserialize, Serialize};
use spitzli_core::id::RecordId;
use std::collections::BTreeMap;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetainedVolume {
    pub mount_path: String,
    pub size_bytes: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutoscalingPolicy {
    pub min_replicas: u16,
    pub max_replicas: u16,
    #[serde(rename = "targetCPUUtilizationPercent")]
    pub target_cpu_utilization_percent: u8,
}
impl AutoscalingPolicy {
    pub fn validate(&self) -> Result<()> {
        if self.min_replicas == 0
            || self.min_replicas > self.max_replicas
            || self.max_replicas > 20
            || !(10..=90).contains(&self.target_cpu_utilization_percent)
        {
            return Err(ExecutionError::Invalid("automatic scaling policy"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeSpec {
    pub version: u8,
    pub installation: String,
    pub project_id: RecordId,
    pub resource_id: RecordId,
    pub revision: u64,
    pub image: String,
    pub arguments: Vec<String>,
    pub cpu_millicores: u32,
    pub memory_bytes: u64,
    pub ephemeral_bytes: u64,
    pub replicas: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autoscaling: Option<AutoscalingPolicy>,
    pub port: u16,
    pub health_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_host: Option<String>,
    pub volume: Option<RetainedVolume>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<PluginBinding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<MountReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hosting_binding: Option<HostingBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hosted_access: Option<crate::HostedAccess>,
}
pub fn image_digest(image: &str) -> bool {
    image.len()<=512 && regex::Regex::new(r"^[a-z0-9]+(?:[.-][a-z0-9]+)*(?::[0-9]{1,5})?/(?:[a-z0-9]+(?:[._-][a-z0-9]+)*/)*[a-z0-9]+(?:[._-][a-z0-9]+)*@sha256:[a-f0-9]{64}$").unwrap().is_match(image)
}
/// Probe header authority only; it never selects a network destination.
pub fn valid_health_host(host: &str) -> bool {
    if host.len() > 259 || !regex::Regex::new(r"^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)*(?::[1-9][0-9]{0,4})?$").unwrap().is_match(host) {
        return false;
    }
    let (hostname, port) = host
        .split_once(':')
        .map_or((host, None), |(name, port)| (name, Some(port)));
    if hostname.len() > 253 || port.is_some_and(|port| port.parse::<u16>().is_err()) {
        return false;
    }
    let origin = format!("https://{host}");
    spitzli_core::config::canonical_origin(&origin, true).is_ok_and(|canonical| {
        canonical == origin || (port == Some("443") && canonical == format!("https://{hostname}"))
    })
}
impl RuntimeSpec {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1
            || self.revision == 0
            || self.revision > 9_007_199_254_740_991
            || !regex::Regex::new(r"^[a-z][a-z0-9-]{0,23}$")
                .unwrap()
                .is_match(&self.installation)
            || !image_digest(&self.image)
        {
            return Err(ExecutionError::Invalid("workload identity or digest"));
        }
        if !(10..=64000).contains(&self.cpu_millicores)
            || !(16_777_216..=1_099_511_627_776).contains(&self.memory_bytes)
            || !(1_048_576..=1_099_511_627_776).contains(&self.ephemeral_bytes)
            || self.replicas > 20
            || self.port < 1024
        {
            return Err(ExecutionError::Invalid("resource limits"));
        }
        if self.arguments.len() > 20
            || self
                .arguments
                .iter()
                .any(|a| a.encode_utf16().count() > 512 || a.contains('\0'))
        {
            return Err(ExecutionError::Invalid("arguments"));
        }
        if let Some(path) = &self.health_path
            && (path.len() > 160
                || !regex::Regex::new(r"^/[A-Za-z0-9/_-]*$")
                    .unwrap()
                    .is_match(path))
        {
            return Err(ExecutionError::Invalid("health path"));
        }
        if self
            .health_host
            .as_deref()
            .is_some_and(|host| !valid_health_host(host))
        {
            return Err(ExecutionError::Invalid("health host"));
        }
        if let Some(policy) = &self.autoscaling {
            policy.validate()?;
            if self.volume.is_some()
                || self.plugin.is_some()
                || (self.replicas != 0
                    && !(policy.min_replicas..=policy.max_replicas).contains(&self.replicas))
            {
                return Err(ExecutionError::Invalid("automatic scaling workload"));
            }
        }
        if let Some(volume) = &self.volume
            && (volume.mount_path != "/data"
                || self.replicas > 1
                || volume
                    .size_bytes
                    .is_some_and(|b| !(67_108_864..=1_099_511_627_776).contains(&b)))
        {
            return Err(ExecutionError::Invalid("retained volume"));
        }
        if let Some(access) = &self.hosted_access
            && (self.plugin.is_some()
                || access.environment.as_ref().is_none_or(|env| {
                    env.revision != self.revision || !valid_sha256(&env.source_sha256)
                }))
        {
            return Err(ExecutionError::Invalid("hosted access reference"));
        }
        if let Some(references) = self
            .hosted_access
            .as_ref()
            .and_then(|a| a.service_bindings.as_ref())
        {
            if references.len() > 16 {
                return Err(ExecutionError::Invalid("service binding limit"));
            }
            let mut seen = std::collections::BTreeSet::new();
            for reference in references {
                reference.validate()?;
                if !seen.insert(reference.identity().0) {
                    return Err(ExecutionError::Invalid("duplicate service binding"));
                }
            }
        }
        if let Some(binding) = &self.hosting_binding {
            binding.quota.validate()?;
            binding.final_quota.validate()?;
            if let Some(route) = &binding.public_route {
                route.validate()?;
            }
            if self.plugin.is_some()
                || !self.mounts.is_empty()
                || binding
                    .final_quota
                    .values()
                    .iter()
                    .zip(binding.quota.values())
                    .any(|(end, start)| *end > start)
            {
                return Err(ExecutionError::Invalid("retained hosting binding"));
            }
        }
        if self.mounts.len() > 8 {
            return Err(ExecutionError::Invalid("mount limit"));
        }
        let mut names = std::collections::HashSet::new();
        for mount in &self.mounts {
            if !valid_mount_name(&mount.name) || mount.revision == 0 || !names.insert(&mount.name) {
                return Err(ExecutionError::Invalid("mount reference"));
            }
        }
        if let Some(plugin) = &self.plugin
            && (!valid_sha256(&plugin.manifest_sha256)
                || plugin.plugin_id.len() > 64
                || !regex::Regex::new(r"^[a-z][a-z0-9-]*$")
                    .unwrap()
                    .is_match(&plugin.plugin_id)
                || plugin.version.is_empty()
                || plugin.version.len() > 40
                || plugin.runtime_generation == 0
                || plugin.runtime_generation > 9_007_199_254_740_991)
        {
            return Err(ExecutionError::Invalid("plugin binding"));
        }
        Ok(())
    }
    pub fn name(&self) -> String {
        format!("wd-{}-{}", self.installation, self.resource_id)
    }
    pub fn namespace(&self) -> String {
        self.name()
    }
    pub fn key(&self) -> String {
        format!(
            "{}/{}/{}",
            self.installation, self.project_id, self.resource_id
        )
    }
    pub fn labels(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("webdock.dev/managed-by".into(), "webdock".into()),
            ("webdock.dev/installation".into(), self.installation.clone()),
            ("webdock.dev/project-id".into(), self.project_id.to_string()),
            (
                "webdock.dev/resource-id".into(),
                self.resource_id.to_string(),
            ),
        ])
    }
    pub fn check_owner(&self, labels: &BTreeMap<String, String>) -> Result<()> {
        if self.labels().iter().any(|(k, v)| labels.get(k) != Some(v)) {
            return Err(ExecutionError::Ownership);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Absent,
    Starting,
    Running,
    Stopped,
    Failed,
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Condition {
    pub kind: String,
    pub satisfied: bool,
    pub reason: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutoscalingObservation {
    pub min_replicas: u16,
    pub max_replicas: u16,
    pub current_replicas: u16,
    pub ready_replicas: u16,
    pub desired_replicas: u16,
    pub metrics_available: bool,
    pub reason: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservedState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autoscaling: Option<AutoscalingObservation>,
    pub resource_id: String,
    pub revision: u64,
    pub phase: Phase,
    pub ready: bool,
    pub observed_at: u64,
    pub runtime_id: Option<String>,
    pub conditions: Vec<Condition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<StorageObservation>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StorageObservation {
    /// None means retained storage exists without a verified byte quota.
    pub volume_bytes: Option<u64>,
    pub retained: bool,
}
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Capabilities {
    pub retained_volumes: bool,
    pub volume_quotas: bool,
    pub health: bool,
    pub logs: bool,
    pub metrics: bool,
    pub replicas: bool,
    pub public_routing: bool,
    pub upgrades: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceMetrics {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<MetricsSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_nanoseconds: Option<u64>,
    pub observed_at: u64,
    pub cpu_nanoseconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_nanocores: Option<u64>,
    pub memory_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricsSource {
    DockerEngine,
    KubernetesMetricsApi,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginBinding {
    pub manifest_sha256: String,
    pub plugin_id: String,
    pub version: String,
    pub runtime_generation: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MountReference {
    pub name: String,
    pub revision: u64,
}
pub fn valid_mount_name(name: &str) -> bool {
    name.len() <= 32
        && regex::Regex::new(r"^[a-z][a-z0-9-]*$")
            .unwrap()
            .is_match(name)
}
pub fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|v| v.is_ascii_hexdigit() && !v.is_ascii_uppercase())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetNode {
    pub id: String,
    pub role: String,
    pub architecture: String,
    pub cpu_millicores: u64,
    pub memory_bytes: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetObservation {
    pub version: String,
    pub nodes: Vec<TargetNode>,
    pub runtime_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<StorageCapacity>,
    #[serde(default)]
    pub environment: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_version: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_bindings_version: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_routing_version: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autoscaling_version: Option<u8>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StorageCapacity {
    pub capacity_bytes: u64,
    pub ready: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceAllowance {
    pub apps: u64,
    pub cpu_millicores: u64,
    pub memory_bytes: u64,
    pub volume_bytes: u64,
    pub ephemeral_bytes: u64,
    pub replicas_per_app: u64,
    pub concurrent_deployments: u64,
}
impl ResourceAllowance {
    pub fn values(&self) -> [u64; 7] {
        [
            self.apps,
            self.cpu_millicores,
            self.memory_bytes,
            self.volume_bytes,
            self.ephemeral_bytes,
            self.replicas_per_app,
            self.concurrent_deployments,
        ]
    }
    pub fn validate(&self) -> Result<()> {
        if self.values().iter().any(|v| *v > 9_007_199_254_740_991) {
            return Err(ExecutionError::Invalid("hosting allowance"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicRoute {
    pub allocation_id: RecordId,
    pub hostname: String,
    pub revision: u64,
}
impl PublicRoute {
    pub fn validate(&self) -> Result<()> {
        if self.revision == 0
            || self.revision > 9_007_199_254_740_991
            || self.hostname.contains(':')
            || self.hostname.split('.').count() < 3
            || !valid_health_host(&self.hostname)
            || self.hostname.parse::<std::net::IpAddr>().is_ok()
        {
            return Err(ExecutionError::Invalid("public route"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostingBinding {
    pub cluster_id: RecordId,
    pub quota: ResourceAllowance,
    pub final_quota: ResourceAllowance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_route: Option<PublicRoute>,
}
