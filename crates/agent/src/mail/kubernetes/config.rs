use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};
use webdock_supervisor::{ExecutionError, Result};
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KubernetesMailConfig {
    pub installation: String,
    pub node: String,
    pub image: String,
    #[serde(default = "gateway")]
    pub gateway_namespace: String,
    #[serde(default)]
    pub retained: BTreeMap<String, RetainedBinding>,
}
fn gateway() -> String {
    "webdock-mail-gateway".into()
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetainedBinding {
    #[serde(rename = "namespaceUID")]
    pub namespace_uid: String,
    #[serde(rename = "deploymentUID")]
    pub deployment_uid: String,
    #[serde(rename = "serviceUID")]
    pub service_uid: String,
    #[serde(rename = "volumeUID")]
    pub volume_uid: String,
    #[serde(rename = "claimUID")]
    pub claim_uid: String,
    #[serde(default)]
    pub bootstrap_secret: Option<RetainedSecret>,
    #[serde(default)]
    pub policies: BTreeMap<String, RetainedPolicy>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetainedSecret {
    pub name: String,
    pub uid: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetainedPolicy {
    pub uid: String,
    pub spec_sha256: String,
}
pub(super) fn dns(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|v| v.is_ascii_lowercase() || v.is_ascii_digit() || v == b'-')
}
fn uid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value)
}
fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}
impl KubernetesMailConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let config: Self = serde_json::from_slice(
            &crate::config::read_private(path, 262144).map_err(|_| ExecutionError::Denied)?,
        )
        .map_err(|_| ExecutionError::Invalid("Mail Kubernetes configuration"))?;
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
        if !dns(&self.installation)
            || self.installation.len() > 24
            || !dns(&self.node)
            || !dns(&self.gateway_namespace)
            || !webdock_supervisor::image_digest(&self.image)
            || self.retained.len() > 256
        {
            return Err(ExecutionError::Invalid("Mail Kubernetes configuration"));
        }
        for (customer, binding) in &self.retained {
            if webdock_core::id::RecordId::parse(customer).is_err()
                || ![
                    &binding.namespace_uid,
                    &binding.deployment_uid,
                    &binding.service_uid,
                    &binding.volume_uid,
                    &binding.claim_uid,
                ]
                .into_iter()
                .all(|v| uid(v))
                || binding.bootstrap_secret.as_ref().is_some_and(|s| {
                    !uid(&s.uid) || !s.name.starts_with("mail-bootstrap-") || !dns(&s.name)
                })
                || binding.policies.len() > 16
                || binding
                    .policies
                    .iter()
                    .any(|(name, p)| !dns(name) || !uid(&p.uid) || !hash(&p.spec_sha256))
            {
                return Err(ExecutionError::Invalid("Mail retained resource binding"));
            }
        }
        Ok(())
    }
}
