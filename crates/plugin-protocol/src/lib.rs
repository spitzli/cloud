//! Versioned, provider-neutral plugin contracts. No database or provider credentials.
mod invocation;
mod manifest;
mod policy;
pub mod storage;
pub use invocation::{InvocationClaims, JobIdentity, verify_invocation};
pub use manifest::{Manifest, validate_value};
pub use policy::{ActionPolicy, Availability};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidInput,
    NotSupported,
    PermissionDenied,
    CapabilityUnavailable,
    Conflict,
    Unavailable,
    DeadlineExceeded,
    NotFound,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginError {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}
impl PluginError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: false,
            request_id: None,
        }
    }
    pub fn denied() -> Self {
        Self::new(ErrorCode::PermissionDenied, "Plugin invocation denied")
    }
}
impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for PluginError {}
pub type Result<T> = std::result::Result<T, PluginError>;
pub const MANIFEST_BYTES: usize = 256 * 1024;
pub const FRAME_BYTES: usize = 32 * 1024;
pub const DOCUMENT_BYTES: usize = 64 * 1024;
pub const CAPABILITIES: &[&str] = &[
    "plugin.apps.read",
    "plugin.apps.manage",
    "plugin.apps.provision",
    "project.read",
    "plugin.data.read",
    "plugin.data.write",
    "plugin.db.read",
    "plugin.db.write",
    "plugin.cache.read",
    "plugin.cache.write",
    "plugin.jobs.read",
    "plugin.jobs.enqueue",
    "plugin.jobs.cancel",
    "plugin.events.emit",
    "plugin.events.subscribe",
    "plugin.storage.read",
    "plugin.storage.write",
    "plugin.secrets.read",
    "plugin.http.read",
    "plugin.http.request",
    "plugin.audit.emit",
    "plugin.workloads",
];
/// Kubernetes kinds (`apiVersion/Kind`) a plugin may declare for its workloads. Everything is
/// namespaced; Core pins the namespace and owner labels, and the agent checks the same list.
pub const WORKLOAD_KINDS: &[&str] = &[
    "apps/v1/Deployment",
    "apps/v1/StatefulSet",
    "v1/Service",
    "v1/Secret",
    "v1/ConfigMap",
    "v1/PersistentVolumeClaim",
    "networking.k8s.io/v1/NetworkPolicy",
    "networking.k8s.io/v1/Ingress",
    "traefik.io/v1alpha1/IngressRoute",
    "traefik.io/v1alpha1/Middleware",
    "traefik.io/v1alpha1/IngressRouteTCP",
    "cert-manager.io/v1/Certificate",
    "postgresql.cnpg.io/v1/Cluster",
    "postgresql.cnpg.io/v1/Pooler",
];
pub const READ_CAPABILITIES: &[&str] = &[
    "plugin.apps.read",
    "plugin.http.read",
    "project.read",
    "plugin.data.read",
    "plugin.db.read",
    "plugin.cache.read",
    "plugin.storage.read",
    "plugin.jobs.read",
    "plugin.secrets.read",
];

/// A Kubernetes quantity in `unit`s of its base, rounded up: `(1 << 20, 1)` turns "512Mi" into MiB,
/// `(1, 1000)` turns "250m" or "0.5" into millicores.
pub fn quantity(value: &serde_json::Value, unit: (u128, u128)) -> Option<u64> {
    let text = match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => return None,
    };
    let split = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let (number, suffix) = text.split_at(split);
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if (whole.is_empty() && fraction.is_empty()) || whole.len() + fraction.len() > 18 {
        return None;
    }
    let digits: u128 = format!("{whole}{fraction}").parse().ok()?;
    let (multiply, divide): (u128, u128) = match suffix {
        "" => (1, 1),
        "n" => (1, 1_000_000_000),
        "u" => (1, 1_000_000),
        "m" => (1, 1000),
        "k" => (1000, 1),
        "M" => (1_000_000, 1),
        "G" => (1_000_000_000, 1),
        "T" => (1_000_000_000_000, 1),
        "Ki" => (1 << 10, 1),
        "Mi" => (1 << 20, 1),
        "Gi" => (1 << 30, 1),
        "Ti" => (1 << 40, 1),
        _ => return None,
    };
    let denominator = 10u128.pow(u32::try_from(fraction.len()).ok()?) * divide * unit.0;
    u64::try_from((digits * multiply * unit.1).div_ceil(denominator)).ok()
}
/// The message of an error the customer may read: what was wrong with their input, what conflicts
/// (a plan limit, an unverified domain) or what is missing. One line of at most 300 characters.
/// Other errors may carry diagnostics and cross process boundaries with a fixed text only.
pub fn public_message(code: ErrorCode, message: &str) -> Option<String> {
    if !matches!(
        code,
        ErrorCode::InvalidInput | ErrorCode::Conflict | ErrorCode::NotFound
    ) {
        return None;
    }
    let line: String = message
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .take(300)
        .collect();
    let line = line.trim();
    (!line.is_empty()).then(|| line.to_owned())
}
/// Quantity units: MiB of bytes, millicores of cores.
pub const MIB: (u128, u128) = (1 << 20, 1);
pub const MILLI: (u128, u128) = (1, 1000);

#[cfg(test)]
mod quantity_tests {
    use super::*;
    #[test]
    fn only_customer_errors_keep_their_words() {
        assert_eq!(
            public_message(
                ErrorCode::Conflict,
                "Your plan allows 2 of these and 2 are in use.\nsecret"
            )
            .as_deref(),
            Some("Your plan allows 2 of these and 2 are in use.")
        );
        assert_eq!(
            public_message(ErrorCode::InvalidInput, &"x".repeat(400)).map(|m| m.len()),
            Some(300)
        );
        assert_eq!(
            public_message(
                ErrorCode::Unavailable,
                "connect to postgres://user:pw@db failed"
            ),
            None
        );
        assert_eq!(public_message(ErrorCode::PermissionDenied, "denied"), None);
        assert_eq!(public_message(ErrorCode::NotFound, " \u{7}"), None);
    }
    use serde_json::json;
    #[test]
    fn quantities_round_up_to_whole_units() {
        assert_eq!(quantity(&json!("512Mi"), MIB), Some(512));
        assert_eq!(quantity(&json!("1Gi"), MIB), Some(1024));
        assert_eq!(quantity(&json!("1G"), MIB), Some(954));
        assert_eq!(quantity(&json!("1.5Gi"), MIB), Some(1536));
        assert_eq!(quantity(&json!("250m"), MILLI), Some(250));
        assert_eq!(quantity(&json!("0.5"), MILLI), Some(500));
        assert_eq!(quantity(&json!(2), MILLI), Some(2000));
        assert_eq!(quantity(&json!("1"), MIB), Some(1));
        assert_eq!(quantity(&json!("2500000n"), MILLI), Some(3));
        assert_eq!(quantity(&json!("10240Ki"), MIB), Some(10));
        for bad in [
            json!("1e3"),
            json!("-1Gi"),
            json!("Mi"),
            json!(""),
            json!(null),
            json!("1Xi"),
        ] {
            assert_eq!(quantity(&bad, MIB), None, "{bad}");
        }
    }
}
pub mod release;
