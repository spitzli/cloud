//! Momentary tenant-only Mail policy. The domain retains its own job and attempt authority.
use super::*;

pub struct MailPolicySnapshot {
    pub customer_id: String,
    pub plugin_id: String,
    pub version: String,
    pub generation: u64,
    pub grant_revision: u64,
    pub transport_revision: u64,
    pub configuration_revision: u64,
    pub configuration: Value,
    pub manifest_sha256: String,
    pub image: String,
    pub registry_url: String,
    pub signer_key_id: String,
    pub binding_sha256: String,
    pub binding: RuntimeBinding,
    pub managed: bool,
    pub transport: Value,
}
impl MailPolicySnapshot {
    pub fn permits_peer(&self, peer: &PeerIdentity) -> bool {
        self.binding.permits(peer)
    }
}
impl HostService {
    /// Requires an already authenticated, signed mail-send-gateway transaction. Keeps all
    /// locks in the caller's transaction; never grants durable authority or commits the handoff.
    pub async fn mail_policy_in(
        &self,
        tx: &DatabaseTransaction,
        customer: &str,
        continuation: bool,
    ) -> Result<MailPolicySnapshot> {
        if customer.starts_with('0')
            || !customer.bytes().all(|b| b.is_ascii_digit())
            || !customer.parse::<i64>().is_ok_and(|n| n > 0)
        {
            return Err(invalid("Invalid customer"));
        }
        let row = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.mail_plugin_policy($1,$2) AS policy",
                [customer.into(), continuation.into()],
            ))
            .await
            .map_err(db_error)?
            .ok_or_else(unavailable)?;
        let policy: Value = row
            .try_get::<Option<Value>>("", "policy")
            .map_err(db_error)?
            .ok_or_else(PluginError::denied)?;
        let release = policy["release"].clone();
        let raw = string(&release, "manifest_bytes")?;
        if digest(raw) != string(&release, "manifest_sha256")? {
            return Err(unavailable());
        }
        let manifest = Manifest::parse(raw.as_bytes())?;
        let managed = policy["installation"]["id"] == "mail";
        manifest.require_host_services(if managed {
            &[
                ("http", 1),
                ("data", 1),
                ("jobs", 1),
                ("native-mail-control", 1),
            ]
        } else {
            &[("http", 1)]
        })?;
        let plugin = string(&policy["installation"], "id")?;
        if manifest.id() != plugin
            || manifest.version() != string(&policy["installation"], "version")?
            || manifest.as_json()["image"] != release["image"]
        {
            return denied();
        }
        let metadata = Metadata {
            release,
            manifest,
            installation: policy["installation"].clone(),
            runtime: policy["runtime"].clone(),
            configuration: policy["configuration"].clone(),
            grant: Value::Null,
        };
        if managed {
            super::native_mail::readable_custody(&metadata)?;
        }
        let binding = if managed {
            self.mail_binding_in(
                &metadata,
                (policy["managedEnrollment"] == true).then(|| policy["enrollment"].clone()),
            )
            .await?
        } else {
            self.binding(plugin, &metadata).await?
        };
        let mut config = metadata.configuration["configuration"]
            .as_object()
            .cloned()
            .ok_or_else(unavailable)?;
        if let Some(overrides) = binding.public_customer_configuration.get(customer) {
            config.extend(overrides.as_object().ok_or_else(unavailable)?.clone());
        }
        let configuration = crate::data::legacy_json_value(&Value::Object(config))?;
        validate_value(
            Some(&metadata.manifest.as_json()["configuration"]),
            &configuration,
        )?;
        if serde_json::to_vec(&configuration)
            .map_err(|_| unavailable())?
            .len()
            > 65536
        {
            return Err(invalid("Mail configuration exceeds limit"));
        }
        let binding_sha256 = digest(
            serde_json::to_vec(&json!({"binding":binding,"configuration":configuration}))
                .map_err(|_| unavailable())?,
        );
        Ok(MailPolicySnapshot {
            customer_id: customer.into(),
            plugin_id: plugin.into(),
            version: string(&metadata.installation, "version")?.into(),
            generation: integer(&metadata.runtime, "generation")? as u64,
            grant_revision: integer(&policy["binding"], "revision")? as u64,
            transport_revision: integer(&policy["transport"], "revision")? as u64,
            configuration_revision: integer(&metadata.configuration, "revision")? as u64,
            configuration,
            manifest_sha256: string(&metadata.release, "manifest_sha256")?.into(),
            image: string(&metadata.release, "image")?.into(),
            registry_url: string(&metadata.release, "registry_url")?.into(),
            signer_key_id: string(&metadata.release, "signer_key_id")?.into(),
            binding_sha256,
            binding,
            managed,
            transport: policy["transport"].clone(),
        })
    }
}
