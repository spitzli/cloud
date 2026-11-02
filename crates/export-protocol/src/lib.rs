//! Typed source identities are locations, never authorizations. Private source bytes use streaming RPC.
use serde::{Deserialize, Serialize};
#[allow(
    clippy::allow_attributes_without_reason,
    clippy::excessive_nesting,
    reason = "Tonic owns generated nesting and lint attributes; handwritten export contracts keep workspace policy."
)]
pub mod wire {
    tonic::include_proto!("webdock.export.v1");
}
pub const MAX_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_BUILD_LOG_BYTES: usize = 24_000;
pub const MAX_BUILD_LOG_GZIP_BYTES: u64 = 64 * 1024;
pub const CHUNK_BYTES: usize = 64 * 1024;
pub const MAX_METADATA: usize = 32768;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Postgres,
    Project,
    Report,
    BuildLog,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceRef {
    pub version: u8,
    pub kind: SourceKind,
    pub id: String,
    pub binding: String,
    pub commit: Option<String>,
}
impl SourceRef {
    pub fn validate(&self) -> Result<(), &'static str> {
        let hex = |s: &str, n| {
            s.len() == n
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        if self.version != 1
            || !decimal(&self.id)
            || !hex(&self.binding, 64)
            || self
                .commit
                .as_ref()
                .is_some_and(|s| self.kind != SourceKind::Project || !hex(s, 40))
        {
            return Err("Invalid source reference");
        }
        Ok(())
    }
    pub fn finished(&self) -> Result<(), &'static str> {
        self.validate()?;
        if self.kind == SourceKind::Project && self.commit.is_none() {
            return Err("Immutable commit required");
        }
        Ok(())
    }
}
pub fn fingerprint(value: &impl Serialize) -> Result<String, &'static str> {
    use sha2::{Digest, Sha256};
    Ok(hex::encode(Sha256::digest(
        serde_json::to_vec(value).map_err(|_| "Invalid export source")?,
    )))
}
pub fn decimal(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 19
        && !id.starts_with('0')
        && id.bytes().all(|b| b.is_ascii_digit())
}
/// Requires the exact canonical native 256-bit credential encoding; never accepts caller passwords.
pub fn readonly_password(database: &str, native: &str) -> Result<String, &'static str> {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use hmac::{Hmac, Mac};
    if !decimal(database) {
        return Err("Invalid export database");
    }
    let key = URL_SAFE_NO_PAD
        .decode(native)
        .map_err(|_| "Native generated credential required")?;
    if key.len() != 32 || URL_SAFE_NO_PAD.encode(&key) != native {
        return Err("Native generated credential required");
    }
    let mut mac =
        Hmac::<sha2::Sha256>::new_from_slice(&key).map_err(|_| "Invalid export credential")?;
    mac.update(b"webdock.database-export.readonly.v1\0");
    mac.update(database.as_bytes());
    Ok(URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

impl std::fmt::Debug for wire::DatabaseMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DatabaseMaterial([confidential])")
    }
}
