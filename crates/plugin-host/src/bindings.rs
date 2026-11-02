use crate::{http::HttpBinding, unavailable};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::os::unix::fs::MetadataExt;
use std::{collections::BTreeMap, path::PathBuf};
use tokio::io::AsyncReadExt;
use webdock_plugin_protocol::Result;
use webdock_plugin_runtime::PeerIdentity;
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum PeerEnrollment {
    Unix {
        uid: u32,
    },
    MutualTls {
        #[serde(rename = "certificateSha256", alias = "certificate_sha256")]
        certificate_sha256: String,
    },
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeBinding {
    /// Operator-approved binary origins, project -> alias -> pinned binding.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub storage: BTreeMap<String, BTreeMap<String, webdock_plugin_http::storage::StorageBinding>>,
    pub generation: u64,
    pub namespace: String,
    pub service: String,
    pub credential_sha256: String,
    pub peers: Vec<PeerEnrollment>,
    #[serde(default)]
    pub public_configuration: BTreeMap<String, Value>,
    #[serde(default)]
    pub projects: BTreeMap<String, BTreeMap<String, HttpBinding>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub public_customer_configuration: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub customers: BTreeMap<String, BTreeMap<String, HttpBinding>>,
    /// The enrolled cluster this plugin's workloads run on (`plugin.workloads`), chosen by the operator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workload_cluster: Option<String>,
    /// Domains whose hosts this plugin's units may route and certify without a customer verifying
    /// them, e.g. `apps.example.eu` for `<app>.apps.example.eu`. Chosen by the operator.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub route_domains: Vec<String>,
}
impl RuntimeBinding {
    pub(crate) fn permits(&self, peer: &PeerIdentity) -> bool {
        self.peers.iter().any(|enrolled| match (enrolled, peer) {
            (PeerEnrollment::Unix { uid: expected }, PeerIdentity::Unix { uid }) => uid == expected,
            (
                PeerEnrollment::MutualTls { certificate_sha256 },
                PeerIdentity::MutualTls { certificates },
            ) => certificates.first().is_some_and(|cert| {
                // TLS authenticates the chain at connection time; an already-open
                // channel must also lose authority when its pinned leaf expires.
                cert.len() <= 65536
                    && x509_parser::parse_x509_certificate(cert)
                        .is_ok_and(|(rest, leaf)| rest.is_empty() && leaf.validity().is_valid())
                    && hex::encode(Sha256::digest(cert)) == *certificate_sha256
            }),
            _ => false,
        })
    }
}
/// Loaded on every authority check so enrollment and binding revocation take effect immediately.
#[async_trait]
pub trait RuntimeBindings: Send + Sync {
    async fn runtime(&self, plugin_id: &str) -> Result<Option<RuntimeBinding>>;
    /// Consume enrollment already locked by the caller; never reacquire the worker pool.
    async fn runtime_from_enrollment(
        &self,
        plugin: &str,
        managed: Option<Value>,
    ) -> Result<Option<RuntimeBinding>> {
        if managed.is_some() {
            return Err(unavailable());
        }
        self.runtime(plugin).await
    }
}
pub struct FileBindings {
    path: PathBuf,
}
impl FileBindings {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    runtimes: BTreeMap<String, RuntimeBinding>,
}
#[async_trait]
impl RuntimeBindings for FileBindings {
    async fn runtime(&self, plugin_id: &str) -> Result<Option<RuntimeBinding>> {
        if !self.path.is_absolute() {
            return Err(unavailable());
        }
        let file = tokio::fs::OpenOptions::new()
            .read(true)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(&self.path)
            .await
            .map_err(|_| unavailable())?;
        let metadata = file.metadata().await.map_err(|_| unavailable())?;
        if !metadata.is_file()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o077 != 0
            || metadata.len() > 1024 * 1024
        {
            return Err(unavailable());
        }
        let mut raw = Vec::new();
        file.take(1024 * 1024 + 1)
            .read_to_end(&mut raw)
            .await
            .map_err(|_| unavailable())?;
        if raw.len() > 1024 * 1024 {
            return Err(unavailable());
        }
        let file: File = serde_json::from_slice(&raw).map_err(|_| unavailable())?;
        Ok(file.runtimes.get(plugin_id).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_pinned_but_expired_tls_leaf_is_not_current_authority() {
        let mut parameters = rcgen::CertificateParams::new(vec!["plugin.invalid".into()]).unwrap();
        parameters.not_before = rcgen::date_time_ymd(2000, 1, 1);
        parameters.not_after = rcgen::date_time_ymd(2001, 1, 1);
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = parameters.self_signed(&key).unwrap();
        let fingerprint = hex::encode(Sha256::digest(cert.der()));
        let binding = RuntimeBinding {
            storage: BTreeMap::new(),
            generation: 1,
            namespace: "fixture".into(),
            service: "plugin".into(),
            credential_sha256: "a".repeat(64),
            peers: vec![PeerEnrollment::MutualTls {
                certificate_sha256: fingerprint,
            }],
            public_configuration: BTreeMap::new(),
            projects: BTreeMap::new(),
            public_customer_configuration: BTreeMap::new(),
            customers: BTreeMap::new(),
            workload_cluster: None,
            route_domains: Vec::new(),
        };
        assert!(!binding.permits(&PeerIdentity::MutualTls {
            certificates: vec![cert.der().to_vec()]
        }));
    }
}
