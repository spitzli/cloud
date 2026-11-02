//! Native API keys: only hashes are retained; their tenant and optional sender domain are immutable.
use crate::{
    Error, MailDependencies, Result,
    authority::{self, Purpose},
    outbox::{field, one, run},
    proof::valid_id,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sea_orm::{DatabaseTransaction, QueryResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use webdock_browser_session::StoredAuthority;
use webdock_identity::{Principal, RlsSigner};
pub(crate) enum KeyFailure {
    Invalid,
    Denied,
    Unavailable,
}
impl From<Error> for KeyFailure {
    fn from(error: Error) -> Self {
        match error {
            Error::Invalid => Self::Invalid,
            _ => Self::Unavailable,
        }
    }
}
impl From<KeyFailure> for Error {
    fn from(error: KeyFailure) -> Self {
        match error {
            KeyFailure::Invalid => Self::Invalid,
            _ => Self::Unavailable,
        }
    }
}
pub struct NativeKeys {
    pub(crate) dependencies: MailDependencies,
    pub(crate) signer: RlsSigner,
    secret: String,
    pub(crate) enabled: bool,
}
#[derive(Serialize)]
pub struct KeyAccess {
    #[serde(rename = "customerID")]
    pub customer_id: String,
    #[serde(rename = "keyID")]
    pub key_id: String,
    #[serde(rename = "domainID")]
    pub domain_id: Option<String>,
    pub restricted: bool,
}
#[derive(Clone, Deserialize)]
#[serde(tag = "action", rename_all = "lowercase", deny_unknown_fields)]
pub enum KeyCommand {
    Create {
        label: String,
        #[serde(rename = "domainID", default, deserialize_with = "present_domain")]
        domain_id: Option<String>,
    },
    Revoke {
        #[serde(rename = "keyID")]
        key_id: String,
    },
}
fn present_domain<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}
/// Only creation exposes plaintext once; callers cannot mistake a revoked key for a new token.
#[derive(Serialize)]
#[serde(untagged)]
pub enum KeyChange {
    Created { id: String, token: String },
    Revoked { id: String },
}

fn token_hash(value: &str) -> Result<String> {
    let suffix = value.strip_prefix("wd_mail_").ok_or(Error::Invalid)?;
    if suffix.len() != 43
        || !suffix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(Error::Invalid);
    }
    Ok(hex::encode(Sha256::digest(value.as_bytes())))
}
impl NativeKeys {
    pub async fn new(dependencies: MailDependencies, secret: &str, enabled: bool) -> Result<Self> {
        authority::validate_pools(&dependencies).await?;
        Ok(Self {
            dependencies,
            signer: RlsSigner::new(secret, "webdock_auth_runtime").map_err(|_| Error::Invalid)?,
            secret: secret.into(),
            enabled,
        })
    }
    pub async fn manage(
        &self,
        principal: &Principal,
        authority: &StoredAuthority,
        customer: &str,
        command: KeyCommand,
    ) -> Result<KeyChange> {
        if !self.enabled || !valid_id(customer) {
            return Err(Error::Invalid);
        }
        let event = self
            .dependencies
            .ids
            .allocate()
            .await
            .map_err(|_| Error::Unavailable)?
            .to_string();
        let new_id = if matches!(command, KeyCommand::Create { .. }) {
            Some(
                self.dependencies
                    .ids
                    .allocate()
                    .await
                    .map_err(|_| Error::Unavailable)?
                    .to_string(),
            )
        } else {
            None
        };
        let authorized = if matches!(command, KeyCommand::Revoke { .. }) {
            authority::customer_for_revocation(
                &self.dependencies,
                &self.signer,
                principal,
                authority,
                customer,
            )
            .await?
        } else {
            authority::customer(
                &self.dependencies,
                &self.signer,
                &principal.user_id,
                &principal.session_id,
                authority,
                customer,
                true,
            )
            .await?
        };
        let tx = &authorized.business;
        one(
            tx,
            "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
            vec![format!("mail-keys:{customer}").into()],
        )
        .await?;
        let (result, target, action) = match command {
            KeyCommand::Revoke { key_id } => {
                if !valid_id(&key_id) {
                    return Err(Error::Invalid);
                }
                let revoked = run(
                    tx,
                    "UPDATE webdock_mail.api_key SET revoked_at=coalesce(revoked_at,now())
                     WHERE id=$1 AND customer_id=$2",
                    vec![key_id.clone().into(), customer.into()],
                )
                .await?;
                if !revoked {
                    return Err(Error::Unavailable);
                }
                (
                    KeyChange::Revoked { id: key_id.clone() },
                    key_id,
                    "native-mail-key-revoke",
                )
            }
            KeyCommand::Create { label, domain_id } => {
                let label = label.trim();
                if label.is_empty()
                    || label.encode_utf16().count() > 100
                    || label.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}')
                    || domain_id.as_ref().is_some_and(|v| !valid_id(v))
                {
                    return Err(Error::Invalid);
                }
                let active_keys = one(
                    tx,
                    "SELECT count(*)::bigint AS count FROM webdock_mail.api_key
                     WHERE customer_id=$1 AND revoked_at IS NULL",
                    vec![customer.into()],
                )
                .await?
                .ok_or(Error::Unavailable)?;
                let count: i64 = field(&active_keys, "count")?;
                if count >= 100 {
                    return Err(Error::Unavailable);
                }
                if let Some(domain) = &domain_id {
                    one(
                        tx,
                        "SELECT id FROM webdock_auth.mail_sender_domain
                         WHERE deleted_at IS NULL AND id=$1 AND customer_id=$2
                         AND status='ownership_verified' FOR SHARE",
                        vec![domain.clone().into(), customer.into()],
                    )
                    .await?
                    .ok_or(Error::Unavailable)?;
                }
                one(
                    tx,
                    "SELECT id FROM webdock_auth.platform_settings
                     WHERE id=true AND settings->>'mailEnabled'='true' FOR SHARE",
                    vec![],
                )
                .await?
                .ok_or(Error::Unavailable)?;
                let mut bytes = [0u8; 32];
                getrandom::getrandom(&mut bytes).map_err(|_| Error::Unavailable)?;
                let token = format!("wd_mail_{}", URL_SAFE_NO_PAD.encode(bytes));
                let id = new_id.ok_or(Error::Unavailable)?;
                run(
                    tx,
                    "INSERT INTO webdock_mail.api_key
                     (id,customer_id,label,token_hash,key_suffix,domain_restricted,domain_id,created_by)
                     VALUES($1,$2,$3,$4,$5,$6,$7,$8)",
                    vec![
                        id.clone().into(), customer.into(), label.into(),
                        token_hash(&token)?.into(), token[token.len()-4..].into(),
                        domain_id.is_some().into(), domain_id.into(), principal.user_id.clone().into(),
                    ],
                ).await?;
                (
                    KeyChange::Created {
                        id: id.clone(),
                        token,
                    },
                    id,
                    "native-mail-key-create",
                )
            }
        };
        run(
            tx,
            "INSERT INTO webdock_auth.access_event(id,actor_id,action,target_id,outcome)
             VALUES($1,$2,$3,$4,'succeeded')",
            vec![
                event.into(),
                principal.user_id.clone().into(),
                action.into(),
                target.into(),
            ],
        )
        .await?;
        authorized
            .business
            .commit()
            .await
            .map_err(|_| Error::Unavailable)?;
        authorized
            .auth
            .rollback()
            .await
            .map_err(|_| Error::Unavailable)?;
        Ok(result)
    }
    pub async fn authenticate(&self, value: &str, sender: Option<&str>) -> Result<KeyAccess> {
        if !self.enabled {
            return Err(Error::Unavailable);
        }
        let tx = authority::worker(
            &self.dependencies,
            &self.secret,
            Purpose::Sending,
            Some(false),
        )
        .await?;
        let result = Self::authenticate_in(&tx, value, sender).await?;
        tx.commit().await.map_err(|_| Error::Unavailable)?;
        Ok(result)
    }
    pub(crate) async fn authenticate_in(
        tx: &DatabaseTransaction,
        value: &str,
        sender: Option<&str>,
    ) -> Result<KeyAccess> {
        Self::authenticate_proof_in(tx, value, sender)
            .await
            .map_err(Error::from)
    }
    pub(crate) async fn authenticate_proof_in(
        tx: &DatabaseTransaction,
        value: &str,
        sender: Option<&str>,
    ) -> std::result::Result<KeyAccess, KeyFailure> {
        let hash = token_hash(value)?;
        let row = one(
            tx,
            "SELECT k.*,d.domain,d.status AS domain_status FROM webdock_mail.api_key k
             LEFT JOIN webdock_auth.mail_sender_domain d
             ON d.deleted_at IS NULL AND d.id=k.domain_id AND d.customer_id=k.customer_id
             WHERE k.token_hash=$1 AND k.revoked_at IS NULL FOR SHARE OF k",
            vec![hash.into()],
        )
        .await?
        .ok_or(KeyFailure::Denied)?;
        authorize_row(tx, &row, sender).await
    }
    pub async fn authorize_id(
        &self,
        customer: &str,
        key: &str,
        sender: Option<&str>,
    ) -> Result<KeyAccess> {
        if !self.enabled {
            return Err(Error::Unavailable);
        }
        let tx = authority::worker(
            &self.dependencies,
            &self.secret,
            Purpose::Sending,
            Some(false),
        )
        .await?;
        let result = Self::authorize_id_in(&tx, customer, key, sender).await?;
        tx.commit().await.map_err(|_| Error::Unavailable)?;
        Ok(result)
    }
    pub(crate) async fn authorize_id_in(
        tx: &DatabaseTransaction,
        customer: &str,
        key: &str,
        sender: Option<&str>,
    ) -> Result<KeyAccess> {
        Self::authorize_id_proof_in(tx, customer, key, sender)
            .await
            .map_err(Error::from)
    }
    pub(crate) async fn authorize_id_proof_in(
        tx: &DatabaseTransaction,
        customer: &str,
        key: &str,
        sender: Option<&str>,
    ) -> std::result::Result<KeyAccess, KeyFailure> {
        if !valid_id(customer) || !valid_id(key) {
            return Err(KeyFailure::Invalid);
        }
        let row = one(
            tx,
            "SELECT k.*,d.domain,d.status AS domain_status FROM webdock_mail.api_key k
             LEFT JOIN webdock_auth.mail_sender_domain d
             ON d.deleted_at IS NULL AND d.id=k.domain_id AND d.customer_id=k.customer_id
             WHERE k.id=$1 AND k.customer_id=$2 AND k.revoked_at IS NULL FOR SHARE OF k",
            vec![key.into(), customer.into()],
        )
        .await?
        .ok_or(KeyFailure::Denied)?;
        authorize_row(tx, &row, sender).await
    }
}
async fn authorize_row(
    tx: &DatabaseTransaction,
    row: &QueryResult,
    sender: Option<&str>,
) -> std::result::Result<KeyAccess, KeyFailure> {
    let customer: String = field(row, "customer_id")?;
    one(
        tx,
        "SELECT c.id FROM webdock_admin.customers c WHERE c.id=$1 AND c.status='active'
         AND EXISTS(SELECT 1 FROM webdock_auth.platform_settings
                    WHERE id=true AND settings->>'mailEnabled'='true') FOR SHARE OF c",
        vec![customer.clone().into()],
    )
    .await?
    .ok_or(KeyFailure::Denied)?;
    let restricted: bool = field(row, "domain_restricted")?;
    let domain_id: Option<String> = field(row, "domain_id")?;
    let domain: Option<String> = field(row, "domain")?;
    let status: Option<String> = field(row, "domain_status")?;
    if restricted && (domain_id.is_none() || status.as_deref() != Some("ownership_verified")) {
        return Err(KeyFailure::Denied);
    }
    if let Some(sender) = sender {
        let normalized =
            crate::message::envelope_address(sender).map_err(|_| KeyFailure::Invalid)?;
        if restricted && normalized.split_once('@').map(|(_, domain)| domain) != domain.as_deref() {
            return Err(KeyFailure::Denied);
        }
    }
    Ok(KeyAccess {
        customer_id: customer,
        key_id: field(row, "id")?,
        domain_id,
        restricted,
    })
}

#[cfg(test)]
mod command_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn key_commands_reject_null_fields_and_undeclared_authority() {
        for input in [
            json!({"action":"create","label":"test","domainID":null}),
            json!({"action":"create","label":null}),
            json!({"action":"create","label":"test","customerID":"123"}),
            json!({"action":"revoke","keyID":"456","token":"forged"}),
            json!({"action":"revoke","keyID":null}),
            json!({"action":"unknown"}),
        ] {
            assert!(serde_json::from_value::<KeyCommand>(input).is_err());
        }
        assert!(matches!(
            serde_json::from_value::<KeyCommand>(json!({"action":"create","label":"test"}))
                .unwrap(),
            KeyCommand::Create {
                domain_id: None,
                ..
            }
        ));
        assert!(matches!(
            serde_json::from_value::<KeyCommand>(
                json!({"action":"create","label":"test","domainID":"789"})
            ).unwrap(),
            KeyCommand::Create { domain_id: Some(id), .. } if id == "789"
        ));
    }

    #[test]
    fn typed_results_preserve_retained_wire_shapes() {
        assert_eq!(
            serde_json::to_value(KeyChange::Created {
                id: "456".into(),
                token: "once".into(),
            })
            .unwrap(),
            json!({"id":"456","token":"once"})
        );
        assert_eq!(
            serde_json::to_value(KeyChange::Revoked { id: "456".into() }).unwrap(),
            json!({"id":"456"})
        );
    }
}
