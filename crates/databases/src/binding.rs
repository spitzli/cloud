use crate::{Result, invalid};
use serde::Serialize;
use std::collections::BTreeMap;
#[derive(Clone)]
pub struct DatabaseBindingRequest {
    pub database_id: String,
    pub project_id: String,
    pub customer_id: String,
    pub cluster_id: String,
    pub binding_id: String,
    pub variable: String,
}
/// Confidential hydration result: never persisted into operation commands, audit events or outboxes.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseBindingMaterial {
    pub credential_revision: i32,
    pub environment: BTreeMap<String, String>,
    pub runtime: DatabaseBindingRuntime,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseBindingRuntime {
    pub id: String,
    pub revision: i32,
    #[serde(rename = "backendID")]
    pub backend_id: String,
    pub ca_certificate: String,
}
pub fn database_connection_url(
    host: &str,
    port: u16,
    database: &str,
    username: &str,
    password: &str,
    binding_id: &str,
) -> Result<String> {
    if host.len() < 2
        || host.len() > 253
        || !host.as_bytes()[0].is_ascii_alphanumeric()
        || !host
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
        || port == 0
        || binding_id.is_empty()
        || binding_id.starts_with('0')
        || !binding_id.bytes().all(|b| b.is_ascii_digit())
        || binding_id.parse::<i64>().is_err()
    {
        return Err(invalid());
    }
    let mut url = url::Url::parse(&format!("postgresql://{host}:{port}")).map_err(|_| invalid())?;
    url.set_username(username).map_err(|_| invalid())?;
    url.set_password(Some(password)).map_err(|_| invalid())?;
    url.set_path(&format!("/{database}"));
    url.query_pairs_mut()
        .append_pair("sslmode", "verify-full")
        .append_pair(
            "sslrootcert",
            &format!("/var/run/webdock/services/{binding_id}/ca.crt"),
        );
    Ok(url.to_string())
}
