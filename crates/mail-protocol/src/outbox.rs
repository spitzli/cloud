//! Canonical native send DTOs. Field order and private token serialization are retained.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PayloadFormat {
    Request,
    Mime,
}
impl PayloadFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Mime => "mime",
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SendLease {
    pub id: String,
    #[serde(rename = "customerID")]
    pub customer_id: String,
    #[serde(rename = "sourceID")]
    pub source_id: String,
    pub payload_ref: String,
    pub payload_bytes: i32,
    pub request_hash: String,
    pub payload_format: PayloadFormat,
    pub source_context: Value,
    pub route: Value,
    pub envelope: Value,
    pub lease_token: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase", deny_unknown_fields)]
pub enum Receipt {
    Accepted {
        #[serde(rename = "providerID")]
        provider_id: String,
    },
    Rejected {},
    Unknown {},
}
impl Receipt {
    pub fn state(&self) -> &'static str {
        match self {
            Self::Accepted { .. } => "accepted",
            Self::Rejected {} => "rejected",
            Self::Unknown {} => "unknown",
        }
    }
    pub fn provider(&self) -> Option<&str> {
        match self {
            Self::Accepted { provider_id } => Some(provider_id),
            _ => None,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PayloadLease {
    pub id: String,
    #[serde(rename = "customerID")]
    pub customer_id: String,
    pub payload_ref: String,
    pub upload_token: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preparation {
    pub metadata_sha256: String,
    pub mime_sha256: String,
    pub mime_bytes: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupItem {
    pub id: String,
    #[serde(rename = "customerID")]
    pub customer_id: String,
    #[serde(rename = "ref")]
    pub reference: String,
}
