//! Authenticated ephemeral application material. References bind randomized ciphertext,
//! never public hashes of plaintext credentials. Command serialization skips material.
use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostedAccess {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_bindings: Option<Vec<ServiceBindingReference>>,
    pub environment: Option<EnvironmentReference>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentReference {
    pub revision: u64,
    pub source_sha256: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeMaterial {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_bindings: Option<Vec<ServiceBindingMaterial>>,
    pub operation_id: String,
    pub request_sha256: String,
    pub environment: Option<EnvironmentMaterial>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentMaterial {
    pub source_sha256: String,
    pub values: BTreeMap<String, String>,
}
impl EnvironmentMaterial {
    pub fn validate(&self) -> Result<()> {
        if !valid_sha256(&self.source_sha256) || self.values.len() > 32 {
            return Err(ExecutionError::Invalid("environment material"));
        }
        let key_pattern = regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").unwrap();
        let mut bytes = 0;
        for (key, value) in &self.values {
            if key.len() > 128
                || !key_pattern.is_match(key)
                || ["__proto__", "constructor", "prototype"].contains(&key.as_str())
                || value.len() > 4096
                || value.contains('\0')
            {
                return Err(ExecutionError::Invalid("environment material"));
            }
            bytes += key.len() + value.len();
        }
        if bytes > 12000
            || serde_json::to_vec(&self.values)
                .map_err(|_| ExecutionError::Invalid("environment material"))?
                .len()
                > 16000
        {
            return Err(ExecutionError::Invalid("environment material limit"));
        }
        Ok(())
    }
}
impl Command {
    /// Call only for a new Apply after a live authority check. The Core transport
    /// authenticates this envelope; hashes here bind it to the persisted operation.
    pub fn verify_material(&self) -> Result<()> {
        self.validate()?;
        match (&self.spec.hosted_access, &self.material) {
            (None, None) => Ok(()),
            (Some(access), Some(material)) => {
                if material.operation_id != self.operation_id
                    || material.request_sha256 != self.fingerprint()?
                {
                    return Err(ExecutionError::Ownership);
                }
                match (&access.service_bindings, &material.service_bindings) {
                    (None, None) => {}
                    (Some(references), Some(materials)) if references.len() == materials.len() => {
                        for (reference, value) in references.iter().zip(materials) {
                            reference.verify(value)?;
                        }
                    }
                    _ => return Err(ExecutionError::Ownership),
                }
                match (&access.environment, &material.environment) {
                    (Some(reference), Some(environment))
                        if reference.source_sha256 == environment.source_sha256 =>
                    {
                        environment.validate()
                    }
                    _ => Err(ExecutionError::Ownership),
                }
            }
            _ => Err(ExecutionError::Denied),
        }
    }
}

pub const MAX_RUNTIME_MATERIAL_BYTES: usize = 320 * 1024;
