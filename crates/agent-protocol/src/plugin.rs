//! Installation-owned plugin deployment. No customer/project identity, YAML, commands or secrets
//! belong in the stable specification or durable callbacks.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, net::IpAddr};

#[derive(Debug, Clone, Copy)]
pub struct PluginRuntimeError;
impl std::fmt::Display for PluginRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Invalid plugin runtime contract")
    }
}
impl std::error::Error for PluginRuntimeError {}
pub type Result<T> = std::result::Result<T, PluginRuntimeError>;
pub fn normalize_spec_version(version: u32) -> Result<u32> {
    match version {
        0 | 1 => Ok(1),
        2 => Ok(2),
        _ => Err(PluginRuntimeError),
    }
}
fn ensure(value: bool) -> Result<()> {
    if value {
        Ok(())
    } else {
        Err(PluginRuntimeError)
    }
}
pub fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn slug(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !value.ends_with('-')
}
fn record(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 19
        && !value.starts_with('0')
        && value.parse::<i64>().is_ok_and(|id| id > 0)
}
fn number(value: u64) -> bool {
    value > 0 && value <= 9_007_199_254_740_991
}
fn dns(value: &str) -> bool {
    value.len() <= 253
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}
fn cidr(value: &str) -> bool {
    let Some((address, prefix)) = value.split_once('/') else {
        return false;
    };
    let (Ok(ip), Ok(prefix)) = (address.parse::<IpAddr>(), prefix.parse::<u32>()) else {
        return false;
    };
    match ip {
        IpAddr::V4(ip) => {
            (1..=32).contains(&prefix)
                && u32::from(ip) & (u32::MAX >> prefix.checked_rem(32).unwrap_or(0)) == 0
                || prefix == 32
        }
        IpAddr::V6(ip) => {
            (1..=128).contains(&prefix)
                && (prefix == 128 || u128::from(ip) & (u128::MAX >> prefix) == 0)
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRuntimeEgress {
    pub cidr: String,
    pub port: u16,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRuntimeHostAlias {
    pub ip: IpAddr,
    pub hostnames: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRuntimeSpec {
    pub version: u8,
    pub platform_id: String,
    pub plugin_id: String,
    pub runtime_generation: u64,
    pub configuration_revision: u64,
    pub cluster_id: String,
    pub manifest_sha256: String,
    pub material_sha256: String,
    pub image: String,
    pub namespace: String,
    pub service: String,
    pub runtime_host: String,
    pub port: u16,
    pub entry_point: String,
    pub cpu_millicores: u32,
    pub memory_bytes: u64,
    pub egress: Vec<PluginRuntimeEgress>,
    pub host_aliases: Vec<PluginRuntimeHostAlias>,
}
impl PluginRuntimeSpec {
    pub fn namespace_name(platform: &str, plugin: &str) -> String {
        let input = format!("{}:{platform}{}:{plugin}", platform.len(), plugin.len());
        format!("wd-plugin-{}", &sha256(input.as_bytes())[..24])
    }
    pub fn namespace_name_v2(platform: &str, plugin: &str, generation: u64) -> String {
        let input = format!(
            "v2:{}:{platform}{}:{plugin}:{generation}",
            platform.len(),
            plugin.len()
        );
        format!("wd-plugin-{}", &sha256(input.as_bytes())[..24])
    }
    pub fn service_name() -> &'static str {
        "runtime"
    }
    pub fn validate(&self) -> Result<()> {
        ensure(
            matches!(self.version, 1 | 2)
                && slug(&self.platform_id, 24)
                && slug(&self.plugin_id, 64)
                && record(&self.cluster_id)
                && number(self.runtime_generation)
                && number(self.configuration_revision)
                && hash(&self.manifest_sha256)
                && hash(&self.material_sha256),
        )?;
        let Some((repository, digest)) = self.image.split_once("@sha256:") else {
            return Err(PluginRuntimeError);
        };
        ensure(
            self.image.len() <= 512
                && !repository.is_empty()
                && repository.contains('/')
                && repository
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"./:_-".contains(&b))
                && hash(digest)
                && digest != "0".repeat(64),
        )?;
        ensure(
            self.namespace
                == if self.version == 1 {
                    Self::namespace_name(&self.platform_id, &self.plugin_id)
                } else {
                    Self::namespace_name_v2(
                        &self.platform_id,
                        &self.plugin_id,
                        self.runtime_generation,
                    )
                }
                && self.service == Self::service_name()
                && dns(&self.runtime_host)
                && self.runtime_host.contains('.')
                && self.port >= 1024
                && slug(&self.entry_point, 63)
                && (10..=64000).contains(&self.cpu_millicores)
                && (16_777_216..=68_719_476_736).contains(&self.memory_bytes),
        )?;
        ensure(
            !self.egress.is_empty()
                && self.egress.len() <= 16
                && self.egress.iter().all(|e| e.port > 0 && cidr(&e.cidr))
                && self.host_aliases.len() <= 8
                && self.host_aliases.iter().all(|a| {
                    !a.ip.is_unspecified()
                        && !a.ip.is_multicast()
                        && !a.hostnames.is_empty()
                        && a.hostnames.len() <= 8
                        && a.hostnames.iter().all(|h| dns(h))
                }),
        )
    }
    pub fn fingerprint(&self) -> Result<String> {
        self.validate()?;
        Ok(sha256(
            &serde_json::to_vec(self).map_err(|_| PluginRuntimeError)?,
        ))
    }
    pub fn labels(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("webdock.dev/managed-by".into(), "webdock".into()),
            ("webdock.dev/platform".into(), self.platform_id.clone()),
            ("webdock.dev/plugin".into(), self.plugin_id.clone()),
            (
                "webdock.dev/runtime-generation".into(),
                self.runtime_generation.to_string(),
            ),
        ])
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PluginRuntimeAction {
    Apply,
    Suspend,
    Inspect,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRuntimeLease {
    pub operation_id: String,
    pub generation: u64,
    pub token: String,
    pub lease_until: u64,
    pub agent_generation: u64,
    pub action: PluginRuntimeAction,
}
impl PluginRuntimeLease {
    pub fn validate(&self) -> Result<()> {
        ensure(
            record(&self.operation_id)
                && number(self.generation)
                && hash(&self.token)
                && self.lease_until > 0
                && (1..=i32::MAX as u64).contains(&self.agent_generation),
        )
    }
    pub fn current(&self, now: u64) -> Result<()> {
        self.validate()?;
        ensure(self.lease_until > now && self.lease_until <= now.saturating_add(300))
    }
}
pub const MAX_MATERIAL_BYTES: usize = 384 * 1024;
pub const MATERIAL_FILES: [&str; 6] = [
    "runtime.json",
    "manifest.json",
    "runtime.crt",
    "runtime.key",
    "core-ca.crt",
    "storage-ca.crt",
];
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRuntimeMaterial {
    pub files: BTreeMap<String, String>,
}
impl PluginRuntimeMaterial {
    pub fn validate(&self) -> Result<()> {
        ensure(
            self.files.len() == MATERIAL_FILES.len()
                && MATERIAL_FILES.iter().all(|name| {
                    self.files.get(*name).is_some_and(|value| {
                        !value.is_empty()
                            && value.len()
                                <= if *name == "manifest.json" {
                                    262144
                                } else {
                                    65536
                                }
                            && !value.contains('\0')
                    })
                })
                && serde_json::to_vec(self).is_ok_and(|bytes| bytes.len() <= MAX_MATERIAL_BYTES),
        )
    }
    pub fn fingerprint(&self) -> Result<String> {
        self.validate()?;
        Ok(sha256(
            &serde_json::to_vec(self).map_err(|_| PluginRuntimeError)?,
        ))
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRuntimeCheckpoint {
    pub namespace_uid: Option<String>,
    pub secret_uid: Option<String>,
    pub deployment_uid: Option<String>,
    pub service_uid: Option<String>,
    pub network_policy_uid: Option<String>,
    pub route_uid: Option<String>,
}
impl PluginRuntimeCheckpoint {
    pub fn validate(&self) -> Result<()> {
        ensure(
            [
                &self.namespace_uid,
                &self.secret_uid,
                &self.deployment_uid,
                &self.service_uid,
                &self.network_policy_uid,
                &self.route_uid,
            ]
            .into_iter()
            .flatten()
            .all(|v| {
                !v.is_empty()
                    && v.len() <= 128
                    && v.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
            }),
        )
    }
    pub fn complete(&self) -> bool {
        [
            &self.namespace_uid,
            &self.secret_uid,
            &self.deployment_uid,
            &self.service_uid,
            &self.network_policy_uid,
            &self.route_uid,
        ]
        .iter()
        .all(|v| v.is_some())
    }
    pub fn agrees(&self, other: &Self) -> bool {
        [
            &self.namespace_uid,
            &self.secret_uid,
            &self.deployment_uid,
            &self.service_uid,
            &self.network_policy_uid,
            &self.route_uid,
        ]
        .into_iter()
        .zip([
            &other.namespace_uid,
            &other.secret_uid,
            &other.deployment_uid,
            &other.service_uid,
            &other.network_policy_uid,
            &other.route_uid,
        ])
        .all(|(a, b)| a.is_none() || a == b)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PluginRuntimeOutcome {
    Succeeded,
    NeedsReconciliation,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRuntimeCompletion {
    pub outcome: PluginRuntimeOutcome,
    pub checkpoint: PluginRuntimeCheckpoint,
    pub converged: bool,
}
impl PluginRuntimeCompletion {
    pub fn validate(&self) -> Result<()> {
        self.checkpoint.validate()?;
        ensure(
            self.outcome != PluginRuntimeOutcome::Succeeded
                || self.converged && self.checkpoint.complete(),
        )
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRuntimeClaim {
    pub spec: PluginRuntimeSpec,
    pub lease: PluginRuntimeLease,
    pub material: PluginRuntimeMaterial,
    pub checkpoint: Option<PluginRuntimeCheckpoint>,
}
impl PluginRuntimeClaim {
    pub fn validate(&self, now: u64) -> Result<()> {
        self.spec.validate()?;
        self.lease.current(now)?;
        self.material.validate()?;
        if let Some(checkpoint) = &self.checkpoint {
            checkpoint.validate()?;
        }
        ensure(
            self.spec.material_sha256 == self.material.fingerprint()?
                && self.spec.manifest_sha256
                    == sha256(
                        self.material
                            .files
                            .get("manifest.json")
                            .ok_or(PluginRuntimeError)?
                            .as_bytes(),
                    ),
        )
    }
}
