use crate::{DatabaseBindingRequest, Result, invalid};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseBindingRoute {
    pub id: String,
    pub revision: i32,
    #[serde(rename = "customerID")]
    pub customer_id: String,
    #[serde(rename = "projectID")]
    pub project_id: String,
    #[serde(rename = "sourceClusterID")]
    pub source_cluster_id: String,
    pub backend_revision: i32,
    pub hostname: String,
    pub port: u16,
    pub addresses: Vec<IpAddr>,
}
impl DatabaseBindingRoute {
    pub fn validate(&self) -> Result<()> {
        for id in [
            &self.id,
            &self.customer_id,
            &self.project_id,
            &self.source_cluster_id,
        ] {
            crate::contracts::id(id)?;
        }
        if self.revision < 1 || self.backend_revision < 1 {
            return Err(invalid());
        }
        endpoint(&self.hostname, self.port, &self.addresses)
    }
}
/// A reviewed transport manifest, not evidence that a network connection was made.
/// The service binds it to the existing confirmed endpoint and its CA before approval.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseRouteApproval {
    #[serde(rename = "bindingID")]
    pub binding_id: String,
    pub binding_revision: i32,
    #[serde(rename = "databaseID")]
    pub database_id: String,
    pub database_revision: i32,
    #[serde(rename = "backendID")]
    pub backend_id: String,
    pub backend_revision: i32,
    pub credential_revision: i32,
    #[serde(rename = "sourceClusterID")]
    pub source_cluster_id: String,
    #[serde(rename = "customerID")]
    pub customer_id: String,
    #[serde(rename = "projectID")]
    pub project_id: String,
    pub hostname: String,
    pub port: u16,
    pub ca_sha256: String,
    pub addresses: Vec<IpAddr>,
    pub expected_route_revision: Option<i32>,
    pub idempotency_key: String,
}
impl DatabaseRouteApproval {
    pub fn validate(&self) -> Result<()> {
        for id in [
            &self.binding_id,
            &self.database_id,
            &self.backend_id,
            &self.source_cluster_id,
            &self.customer_id,
            &self.project_id,
        ] {
            crate::contracts::id(id)?;
        }
        for revision in [
            self.binding_revision,
            self.database_revision,
            self.backend_revision,
            self.credential_revision,
        ] {
            if !(1..i32::MAX).contains(&revision) {
                return Err(invalid());
            }
        }
        if self
            .expected_route_revision
            .is_some_and(|r| !(1..i32::MAX).contains(&r))
            || self.ca_sha256.len() != 64
            || !self
                .ca_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || !(16..=128).contains(&self.idempotency_key.len())
            || !self
                .idempotency_key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(invalid());
        }
        endpoint(&self.hostname, self.port, &self.addresses)
    }
}
#[derive(Clone)]
pub struct DatabaseRouteRequest {
    pub binding: DatabaseBindingRequest,
    pub binding_revision: i32,
    pub credential_revision: i32,
    pub route_id: String,
    pub route_revision: i32,
}
fn endpoint(host: &str, port: u16, addresses: &[IpAddr]) -> Result<()> {
    if port == 0
        || host.len() > 253
        || host.parse::<IpAddr>().is_ok()
        || !host.contains('.')
        || host.split('.').any(|l| {
            l.is_empty()
                || l.len() > 63
                || l.starts_with('-')
                || l.ends_with('-')
                || !l
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        || addresses.is_empty()
        || addresses.len() > 8
    {
        return Err(invalid());
    }
    let mut seen = std::collections::HashSet::new();
    for address in addresses {
        let forbidden = match address {
            IpAddr::V4(ip) => {
                ip.is_loopback()
                    || ip.is_unspecified()
                    || ip.is_multicast()
                    || ip.is_broadcast()
                    || ip.is_link_local()
            }
            IpAddr::V6(ip) => {
                ip.is_loopback()
                    || ip.is_unspecified()
                    || ip.is_multicast()
                    || ip.is_unicast_link_local()
                    || ip.to_ipv4_mapped().is_some()
            }
        };
        if forbidden || !seen.insert(address) {
            return Err(invalid());
        }
    }
    Ok(())
}
