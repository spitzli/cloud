//! Submission credentials and SMTP policy for already enabled private Mail instances.
use crate::{
    Error, Result,
    outbox::{field, one},
    proof::valid_id,
    secrets::MailCipher,
};
use sea_orm::DatabaseTransaction;
use serde::Deserialize;
use webdock_mail_protocol::valid_digest;

pub struct IncomingConfig {
    pub managed_cluster: String,
    pub hostname_suffix: String,
    pub encryption_key: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Credentials {
    username: String,
    password: String,
    hook_token: String,
}
pub(crate) struct SubmissionConnection {
    pub certificate_sha256: String,
    pub username: String,
    pub password: String,
}
impl IncomingConfig {
    pub(crate) fn validate(&self) -> Result<()> {
        if !valid_id(&self.managed_cluster)
            || self.hostname_suffix.is_empty()
            || self.hostname_suffix.len() > 253
            || self.hostname_suffix.split('.').any(|part| {
                part.is_empty()
                    || part.len() > 63
                    || part.starts_with('-')
                    || part.ends_with('-')
                    || !part
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            })
        {
            return Err(Error::Invalid);
        }
        MailCipher::new(&self.encryption_key)?;
        Ok(())
    }
    pub(crate) async fn connection(
        &self,
        tx: &DatabaseTransaction,
        customer: &str,
        service_revision: &str,
        transport_revision: u64,
    ) -> Result<SubmissionConnection> {
        if !valid_id(customer)
            || !valid_id(service_revision)
            || transport_revision > i64::MAX as u64
        {
            return Err(Error::Invalid);
        }
        let row=one(tx,"SELECT i.encrypted_submission_credentials,i.smtp_certificate_sha256,i.submission_hook_token_hash FROM webdock_mail.instance i JOIN webdock_mail.service s ON s.customer_id=i.customer_id WHERE i.customer_id=$1 AND i.verified_at IS NOT NULL AND i.submission_state='ready' AND s.host_id=$2 AND s.desired_enabled AND s.state='ready' AND s.revision=s.applied_revision AND s.revision=$3::bigint AND i.smtp_certificate_revision=$3::bigint AND i.submission_ready_revision=$3::bigint AND i.submission_transport_revision=$4::bigint FOR SHARE OF s,i",vec![customer.into(),self.managed_cluster.clone().into(),service_revision.into(),(transport_revision as i64).into()]).await?.ok_or(Error::Unavailable)?;
        let ciphertext = field::<Option<String>>(&row, "encrypted_submission_credentials")?
            .ok_or(Error::Unavailable)?;
        let secret: Credentials = MailCipher::new(&self.encryption_key)?
            .open(&ciphertext, &format!("submission:{customer}"))?;
        let certificate =
            field::<Option<String>>(&row, "smtp_certificate_sha256")?.ok_or(Error::Unavailable)?;
        let hook = field::<Option<String>>(&row, "submission_hook_token_hash")?
            .ok_or(Error::Unavailable)?;
        if secret.username != format!("wd-submit@mail-{customer}.{}", self.hostname_suffix)
            || !(16..=128).contains(&secret.password.len())
            || secret.password.chars().any(|c| c < ' ' || c == '\u{7f}')
            || secret.hook_token.len() > 128
            || webdock_mail_protocol::sha256(secret.hook_token.as_bytes()) != hook
            || !valid_digest(&certificate)
        {
            return Err(Error::Unavailable);
        }
        Ok(SubmissionConnection {
            certificate_sha256: certificate,
            username: secret.username,
            password: secret.password,
        })
    }
}
