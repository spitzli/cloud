//! Signed connection-operation scope; live authority and replay checks remain Core's duty.
use crate::{PluginError, Result, record_id, sign, slug, valid_digest, verify};
use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

const DOMAIN: &[u8] = b"webdock.repository.connection.v1\0";

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionAction {
    Begin,
    Exchange,
    Options,
    Finish,
    Abort,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionScope {
    #[serde(rename = "flowID")]
    pub flow_id: String,
    #[serde(rename = "operationID")]
    pub operation_id: String,
    #[serde(rename = "customerID")]
    pub customer_id: String,
    pub subject: String,
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(rename = "pluginID")]
    pub plugin_id: String,
    pub generation: u64,
    pub action: ConnectionAction,
    pub descriptor_sha256: String,
    pub request_sha256: String,
    pub configuration_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionClaims {
    pub v: u8,
    pub purpose: String,
    pub audience: String,
    pub scope: ConnectionScope,
    pub iat: i64,
    pub exp: i64,
}

fn uuid4(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| {
        id.to_string() == value
            && id.get_version_num() == 4
            && id.get_variant() == uuid::Variant::RFC4122
    })
}

impl ConnectionClaims {
    pub fn validate(&self, now: i64) -> Result<()> {
        let scope = &self.scope;
        if self.v != 1
            || self.purpose != "repository-connection"
            || self.audience != "webdock.repository.connection.runtime.v1"
            || !uuid4(&scope.flow_id)
            || !uuid4(&scope.operation_id)
            || !record_id(&scope.customer_id)
            || !record_id(&scope.subject)
            || scope.session_id.is_empty()
            || scope.session_id.len() > 256
            || !scope.session_id.bytes().all(|b| b.is_ascii_graphic())
            || !slug(&scope.plugin_id)
            || !(1..=9_007_199_254_740_991).contains(&scope.generation)
            || ![
                &scope.descriptor_sha256,
                &scope.request_sha256,
                &scope.configuration_sha256,
            ]
            .iter()
            .all(|digest| valid_digest(digest))
            || self.iat > now
            || self.exp <= now
            || !self
                .exp
                .checked_sub(self.iat)
                .is_some_and(|seconds| (1..=60).contains(&seconds))
        {
            return Err(PluginError::denied());
        }
        Ok(())
    }

    pub fn sign(&self, key: &SigningKey) -> Result<String> {
        self.validate(self.iat)?;
        let bytes = serde_json::to_vec(self).map_err(|_| PluginError::denied())?;
        Ok(sign(DOMAIN, &bytes, key))
    }
}

/// Expectations must come from Core's stored operation, never from the token or browser.
/// This proves issuer and exact scope, not current admission or one-use consumption.
pub fn verify_connection(
    token: &str,
    key: &VerifyingKey,
    expected: &ConnectionScope,
    now: i64,
) -> Result<ConnectionClaims> {
    let bytes = verify(DOMAIN, token, key, 16384)?;
    let claims: ConnectionClaims =
        serde_json::from_slice(&bytes).map_err(|_| PluginError::denied())?;
    claims.validate(now)?;
    if claims.scope != *expected {
        return Err(PluginError::denied());
    }
    Ok(claims)
}
