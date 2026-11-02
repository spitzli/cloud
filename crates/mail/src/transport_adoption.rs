//! Adoption of an operator-configured provider. Clients select a tenant, never credentials.
use super::keys::NativeKeys;
use crate::{
    Error, Result, authority,
    outbox::{field, one, run},
    proof::valid_id,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use webdock_browser_session::StoredAuthority;
use webdock_identity::Principal;

pub struct ConfiguredTransport {
    customer: String,
    account: Option<String>,
    encrypted_credentials: String,
}
fn sealed_envelope(value: &str) -> bool {
    if value.len() > 65536 {
        return false;
    }
    let fields = value.split('.').collect::<Vec<_>>();
    if fields.len() != 4 || fields[0] != "v1" {
        return false;
    }
    let decoded = fields[1..]
        .iter()
        .map(|v| {
            URL_SAFE_NO_PAD
                .decode(v)
                .ok()
                .filter(|raw| !raw.is_empty() && URL_SAFE_NO_PAD.encode(raw) == *v)
        })
        .collect::<Option<Vec<_>>>();
    decoded.is_some_and(|v| v[0].len() == 12 && v[1].len() == 16)
}
impl ConfiguredTransport {
    pub fn managed(
        customer: String,
        account: Option<String>,
        encrypted_credentials: String,
    ) -> Result<Self> {
        if !valid_id(&customer)
            || !customer.parse::<i64>().is_ok_and(|v| v > 0)
            || account
                .as_ref()
                .is_some_and(|v| v.is_empty() || v.len() > 256 || v.chars().any(char::is_control))
            || !sealed_envelope(&encrypted_credentials)
        {
            return Err(Error::Invalid);
        }
        Ok(Self {
            customer,
            account,
            encrypted_credentials,
        })
    }
    pub fn customer(&self) -> &str {
        &self.customer
    }
}
fn observation(stage: &'static str) {
    if std::env::var_os("WEBDOCK_RECOVERY_COMPONENT_TEST").as_deref()
        == Some(std::ffi::OsStr::new("1"))
    {
        use std::io::Write;
        let _ = writeln!(
            std::io::stderr().lock(),
            "native_mail_transport stage={stage}"
        );
    }
}
impl NativeKeys {
    pub async fn adopt_configured_transport(
        &self,
        principal: &Principal,
        authority: &StoredAuthority,
        configured: &ConfiguredTransport,
    ) -> Result<()> {
        if !self.enabled {
            return Err(Error::Unavailable);
        }
        let event = self
            .dependencies
            .ids
            .allocate()
            .await
            .map_err(|_| Error::Unavailable)?
            .to_string();
        observation("authorize");
        let authorized = authority::customer(
            &self.dependencies,
            &self.signer,
            &principal.user_id,
            &principal.session_id,
            authority,
            &configured.customer,
            true,
        )
        .await?;
        if !authorized.operator
            || authorized.principal.in_tenant_preview()
            || authorized.principal.is_acting_customer()
        {
            return Err(Error::Unavailable);
        }
        observation("operator_authorized");
        let tx = &authorized.business;
        one(
            tx,
            "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
            vec![format!("mail-transport-config:{}", configured.customer).into()],
        )
        .await?;
        observation("installation");
        let installed=one(tx,"SELECT id FROM webdock_auth.plugin_installation WHERE id='mail' AND kind='external' AND state='enabled' AND runtime='ready' FOR SHARE",vec![]).await?;
        if installed.is_none() {
            return Err(Error::Unavailable);
        }
        observation("transport");
        let old=one(tx,"SELECT mode,state,revision,region,provider_account_id,byok_revision,encrypted_credentials FROM webdock_mail.send_transport WHERE customer_id=$1 FOR UPDATE",vec![configured.customer.clone().into()]).await?;
        if let Some(row) = old {
            if field::<String>(&row, "mode")? != "managed"
                || field::<String>(&row, "state")? != "ready"
                || field::<i64>(&row, "revision")? != 1
                || field::<String>(&row, "region")? != "eu"
                || field::<Option<String>>(&row, "provider_account_id")? != configured.account
                || field::<Option<i64>>(&row, "byok_revision")?.is_some()
                || field::<Option<String>>(&row, "encrypted_credentials")?.as_deref()
                    != Some(configured.encrypted_credentials.as_str())
            {
                return Err(Error::Unavailable);
            }
        } else {
            run(tx,"INSERT INTO webdock_mail.send_transport(customer_id,mode,revision,state,region,provider_account_id,encrypted_credentials) VALUES($1,'managed',1,'ready','eu',$2,$3)",vec![configured.customer.clone().into(),configured.account.clone().into(),configured.encrypted_credentials.clone().into()]).await?;
        }
        observation("binding");
        let binding=one(tx,"SELECT plugin_id,revision,transport_revision,revoked_at IS NOT NULL AS revoked FROM webdock_mail.provider_binding WHERE customer_id=$1 FOR UPDATE",vec![configured.customer.clone().into()]).await?;
        if let Some(row) = binding {
            if field::<String>(&row, "plugin_id")? != "mail"
                || field::<i64>(&row, "revision")? != 1
                || field::<i64>(&row, "transport_revision")? != 1
                || field::<bool>(&row, "revoked")?
            {
                return Err(Error::Unavailable);
            }
        } else {
            run(tx,"INSERT INTO webdock_mail.provider_binding(customer_id,plugin_id,revision,transport_revision) VALUES($1,'mail',1,1)",vec![configured.customer.clone().into()]).await?;
        }
        observation("audit");
        run(tx,"INSERT INTO webdock_auth.access_event(id,actor_id,action,target_id,outcome) VALUES($1,$2,'native-mail-provider-adopt',$3,'succeeded')",vec![event.into(),authorized.principal.user_id.clone().into(),configured.customer.clone().into()]).await?;
        observation("commit");
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
        observation("complete");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_real_sealed_configuration_but_rejects_foreign_ids_and_control_characters() {
        let value = crate::secrets::MailCipher::new(
            &base64::engine::general_purpose::STANDARD.encode([0u8; 32]),
        )
        .unwrap()
        .seal(&serde_json::json!({}), "send-transport:123:1")
        .unwrap();
        assert!(ConfiguredTransport::managed("123".into(), None, value.clone()).is_ok());
        for customer in ["0", "01", "foreign", "-1"] {
            assert!(ConfiguredTransport::managed(customer.into(), None, value.clone()).is_err());
        }
        assert!(
            ConfiguredTransport::managed("123".into(), Some("private\naccount".into()), value)
                .is_err()
        );
    }

    #[test]
    fn rejects_unsealed_private_material_and_invalid_account_bindings() {
        for bad in [
            "plaintext-private-key",
            "v1.a.b.c",
            "v1...",
            "v1.AAAA.AAAA.AAAA",
        ] {
            assert!(ConfiguredTransport::managed("123".into(), None, bad.into()).is_err());
        }
    }
}
