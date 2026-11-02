//! Fixed service destinations and public trust material; no persisted connection credentials.
use crate::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ServiceBindingReference {
    Postgres {
        id: String,
        revision: u64,
        credential_revision: u64,
        #[serde(rename = "backendID")]
        backend_id: String,
        ca_sha256: String,
    },
    Mail {
        id: String,
        revision: u64,
        credential_revision: u64,
        #[serde(rename = "egressCIDRs")]
        egress_cidrs: Vec<String>,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ServiceBindingMaterial {
    Postgres {
        id: String,
        revision: u64,
        credential_revision: u64,
        ca_certificate: String,
    },
    Mail {
        id: String,
        revision: u64,
        credential_revision: u64,
    },
}
impl ServiceBindingReference {
    pub fn identity(&self) -> (&str, u64, u64) {
        match self {
            Self::Postgres {
                id,
                revision,
                credential_revision,
                ..
            }
            | Self::Mail {
                id,
                revision,
                credential_revision,
                ..
            } => (id, *revision, *credential_revision),
        }
    }
    pub fn validate(&self) -> Result<()> {
        let (id, revision, credential) = self.identity();
        if webdock_core::id::RecordId::parse(id).is_err()
            || revision == 0
            || revision > 9007199254740991
            || credential == 0
            || credential > 9007199254740991
        {
            return Err(ExecutionError::Invalid("service binding reference"));
        }
        match self {
            Self::Postgres {
                backend_id,
                ca_sha256,
                ..
            } => {
                if webdock_core::id::RecordId::parse(backend_id).is_err()
                    || !valid_sha256(ca_sha256)
                {
                    return Err(ExecutionError::Invalid("database binding reference"));
                }
            }
            Self::Mail { egress_cidrs, .. } => {
                if egress_cidrs.is_empty() || egress_cidrs.len() > 8 {
                    return Err(ExecutionError::Invalid("mail destinations"));
                }
                let mut seen = std::collections::BTreeSet::new();
                for cidr in egress_cidrs {
                    let network: ipnet::IpNet = cidr
                        .parse()
                        .map_err(|_| ExecutionError::Invalid("mail destination"))?;
                    let address = network.addr();
                    let max = if address.is_ipv4() { 32 } else { 128 };
                    if network.prefix_len() != max
                        || address.is_loopback()
                        || address.is_unspecified()
                        || address.is_multicast()
                        || !seen.insert(network)
                    {
                        return Err(ExecutionError::Invalid("mail destination"));
                    }
                }
            }
        }
        Ok(())
    }
    pub fn verify(&self, material: &ServiceBindingMaterial) -> Result<()> {
        self.validate()?;
        if self.identity() != material.identity() {
            return Err(ExecutionError::Ownership);
        }
        match (self, material) {
            (Self::Mail { .. }, ServiceBindingMaterial::Mail { .. }) => Ok(()),
            (
                Self::Postgres { ca_sha256, .. },
                ServiceBindingMaterial::Postgres { ca_certificate, .. },
            ) => {
                if ca_certificate.len() > 16384
                    || format!("{:x}", Sha256::digest(ca_certificate.as_bytes())) != *ca_sha256
                {
                    return Err(ExecutionError::Ownership);
                }
                let mut count = 0;
                for pem in x509_parser::pem::Pem::iter_from_buffer(ca_certificate.as_bytes()) {
                    let pem = pem.map_err(|_| ExecutionError::Invalid("service CA"))?;
                    if pem.label != "CERTIFICATE" {
                        return Err(ExecutionError::Invalid("service CA"));
                    }
                    pem.parse_x509()
                        .map_err(|_| ExecutionError::Invalid("service CA"))?;
                    count += 1;
                }
                if count == 0 {
                    return Err(ExecutionError::Invalid("service CA"));
                }
                Ok(())
            }
            _ => Err(ExecutionError::Ownership),
        }
    }
}
impl ServiceBindingMaterial {
    pub fn identity(&self) -> (&str, u64, u64) {
        match self {
            Self::Postgres {
                id,
                revision,
                credential_revision,
                ..
            }
            | Self::Mail {
                id,
                revision,
                credential_revision,
                ..
            } => (id, *revision, *credential_revision),
        }
    }
}
