//! Mailboxes on the customer's shared engine, on one of the project's sending domains.
use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewMailbox {
    domain_id: String,
    local_part: String,
    #[serde(default)]
    name: String,
}
pub const QUOTA_MB: i32 = 1024;
fn local(raw: &str) -> Result<String> {
    let value = raw.trim().to_ascii_lowercase();
    let parts_ok = value.split(['.', '_', '+', '-']).all(|p| {
        !p.is_empty()
            && p.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    });
    if value.is_empty() || value.len() > 64 || !parts_ok {
        return Err(invalid("Use letters, digits and . _ + - for the address"));
    }
    Ok(value)
}
impl Plugin {
    fn engines(&self) -> Result<&engine::Engines> {
        self.engines.as_ref().ok_or_else(|| {
            PluginError::new(ErrorCode::NotSupported, "Mailboxes are not available here")
        })
    }
    /// The engine's admin password, recorded on explicit incoming activation.
    async fn engine_password(&self, customer: &str) -> Result<String> {
        let id = Uuid::new_v4();
        let fresh = hex::encode(seal::random::<24>());
        let engine = self
            .store
            .engine(customer, &id, &self.sealer.seal(&id, &fresh))
            .await
            .map_err(store_error)?;
        self.sealer
            .open(&engine.id, &engine.admin_sealed)
            .ok_or_else(|| unavailable("unseal", engine.id))
    }
    async fn existing_client(&self, customer: &str) -> Result<Option<engine::Client>> {
        let Some(engines) = self.engines.as_ref() else {
            return Ok(None);
        };
        let Some(engine) = self
            .store
            .find_engine(customer)
            .await
            .map_err(store_error)?
        else {
            return Ok(None);
        };
        if !engine.desired_enabled || engine.state != "ready" {
            return Ok(None);
        }
        let admin = self
            .sealer
            .open(&engine.id, &engine.admin_sealed)
            .ok_or_else(|| unavailable("unseal", engine.id))?;
        Ok(Some(engines.client(customer, &admin)))
    }
    pub(crate) async fn create_mailbox(
        self: &Arc<Self>,
        claims: &InvocationClaims,
        input: Value,
    ) -> Result<Value> {
        self.engines()?;
        let input: NewMailbox = parse(input)?;
        let domain = self.domain(claims, &input.domain_id).await?;
        if !domain.verified && !self.settings.trust_unverified_domains {
            return Err(PluginError::new(
                ErrorCode::Conflict,
                "Verify the domain before adding mailboxes",
            ));
        }
        let name = input.name.trim();
        if name.chars().count() > 80 || name.chars().any(char::is_control) {
            return Err(invalid("Invalid display name"));
        }
        let local_part = local(&input.local_part)?;
        let _serial = self.provisioning.lock().await;
        self.require_migrated().await?;
        self.engine_password(&claims.customer_id).await?;
        let engine = self
            .store
            .find_engine(&claims.customer_id)
            .await
            .map_err(store_error)?
            .ok_or_else(missing)?;
        if !engine.desired_enabled {
            return Err(PluginError::new(
                ErrorCode::Conflict,
                "Activate incoming mail before creating a mailbox",
            ));
        }
        let id = Uuid::new_v4();
        let mailbox = store::Mailbox {
            id,
            project_id: claims.project_id.clone(),
            customer_id: claims.customer_id.clone(),
            domain_id: domain.id,
            domain: domain.domain.clone(),
            local_part,
            display_name: name.into(),
            password_sealed: self.sealer.seal(&id, &seal::password()),
            account_id: None,
            quota_mb: QUOTA_MB,
            state: "creating".into(),
            error: None,
            created_at: String::new(),
        };
        if !self
            .store
            .add_mailbox(&mailbox)
            .await
            .map_err(store_error)?
        {
            return Err(PluginError::new(
                ErrorCode::Conflict,
                "This address already exists",
            ));
        }
        let address = format!("{}@{}", mailbox.local_part, mailbox.domain);
        self.provision(mailbox);
        Ok(json!({"resource":{"id":id.to_string(),"name":address}}))
    }
    /// Runs detached: the first mailbox of a customer sets up its engine, which takes a while.
    pub(crate) fn provision(self: &Arc<Self>, mailbox: store::Mailbox) {
        let plugin = Arc::clone(self);
        tokio::spawn(async move {
            let _serial = plugin.provisioning.lock().await;
            let Ok(recovery) = plugin.store.recovery_effect().await else {
                return;
            };
            let mut provider_started = false;
            let outcome = async {
                let engines = plugin.engines().map_err(|e| e.message)?;
                let admin = plugin
                    .engine_password(&mailbox.customer_id)
                    .await
                    .map_err(|e| e.message)?;
                let engine = plugin
                    .store
                    .find_engine(&mailbox.customer_id)
                    .await
                    .map_err(|_| "engine registry")?
                    .ok_or("engine registry")?;
                if !engine.desired_enabled {
                    return Ok(None);
                }
                provider_started = true;
                let engine_result = engines
                    .provision(&mailbox.customer_id, &admin)
                    .await
                    .map_err(|_| "engine provisioning failed".to_owned());
                if let Err(error) = engine_result {
                    plugin
                        .store
                        .engine_state(&mailbox.customer_id, "failed")
                        .await
                        .map_err(|_| "engine state")?;
                    return Err(error);
                }
                plugin
                    .store
                    .engine_state(&mailbox.customer_id, "ready")
                    .await
                    .map_err(|e| e.to_string())?;
                let client = engines.client(&mailbox.customer_id, &admin);
                let domain = client
                    .domain(&mailbox.domain)
                    .await
                    .map_err(|e| e.to_string())?;
                let password = plugin
                    .sealer
                    .open(&mailbox.id, &mailbox.password_sealed)
                    .ok_or("unseal")?;
                let quota = u64::try_from(mailbox.quota_mb).unwrap_or(0) * 1_048_576;
                client
                    .create_mailbox(&domain, &mailbox.local_part, &password, quota)
                    .await
                    .map(Some)
                    .map_err(|_| "mailbox provisioning failed".to_owned())
            }
            .await;
            let provider_completed = outcome.is_ok();
            let stored = match outcome {
                Ok(Some(account)) => plugin.store.mailbox_ready(&mailbox.id, &account).await,
                Ok(None) => {
                    let _ = recovery.finish().await;
                    return;
                }
                Err(error) => {
                    let _ = error;
                    eprintln!("mail_plugin_mailbox: provisioning failed");
                    plugin
                        .store
                        .mailbox_failed(&mailbox.id, "The mailbox could not be set up")
                        .await
                }
            };
            if let Err(error) = stored {
                let _ = error;
                eprintln!("mail_plugin_store: mailbox state failed");
                if provider_started {
                    return; // Keep custody when CE completed but its private outcome was not saved.
                }
            }
            if provider_started && !provider_completed {
                eprintln!("mail_plugin_recovery_effect_uncertain");
                return;
            }
            if recovery.finish().await.is_err() {
                eprintln!("mail_plugin_recovery_effect_uncertain");
            }
        });
    }
    /// Reconcile retained engines and finish interrupted mailboxes after runtime restart.
    pub async fn resume(self: &Arc<Self>) -> std::result::Result<(), store::Error> {
        if self.store.recovery_effect_count().await? != 0 {
            // A restart cannot establish what a previous CE request did.
            eprintln!("mail_plugin_recovery_effect_uncertain");
            return Ok(());
        }
        if self.engines.is_some() && !self.store.legacy_engines().await? {
            for engine in self.store.engines().await? {
                self.reconcile(engine.customer_id);
            }
            for mailbox in self.store.pending_mailboxes().await? {
                self.provision(mailbox);
            }
        }
        Ok(())
    }
    async fn require_migrated(&self) -> Result<()> {
        if self.store.legacy_engines().await.map_err(store_error)? {
            return Err(PluginError::new(
                ErrorCode::Conflict,
                "Incoming mail requires operator migration of retained project engines",
            ));
        }
        Ok(())
    }
    pub(crate) async fn incoming_state(&self, customer: &str) -> Result<Value> {
        if self.store.legacy_engines().await.map_err(store_error)? {
            return Ok(json!({"state":"migration-required","desiredEnabled":false}));
        }
        Ok(
            match self
                .store
                .find_engine(customer)
                .await
                .map_err(store_error)?
            {
                Some(engine) => {
                    json!({"state":engine.state,"desiredEnabled":engine.desired_enabled})
                }
                None => json!({"state":"disabled","desiredEnabled":false}),
            },
        )
    }
    pub(crate) async fn incoming(
        self: &Arc<Self>,
        claims: &InvocationClaims,
        input: Value,
        enabled: bool,
    ) -> Result<Value> {
        self.engines()?;
        let target: Target = parse(input)?;
        self.domain(claims, &target.resource_id).await?;
        self.require_migrated().await?;
        if enabled {
            self.engine_password(&claims.customer_id).await?;
        }
        let Some(_) = self
            .store
            .find_engine(&claims.customer_id)
            .await
            .map_err(store_error)?
        else {
            return Ok(json!({}));
        };
        self.store
            .engine_enabled(&claims.customer_id, enabled)
            .await
            .map_err(store_error)?;
        self.reconcile(claims.customer_id.clone());
        if enabled {
            for mailbox in self
                .store
                .pending_mailboxes()
                .await
                .map_err(store_error)?
                .into_iter()
                .filter(|m| m.customer_id == claims.customer_id)
            {
                self.provision(mailbox);
            }
        }
        Ok(json!({}))
    }
    fn reconcile(self: &Arc<Self>, customer: String) {
        let plugin = Arc::clone(self);
        tokio::spawn(async move {
            let _serial = plugin.provisioning.lock().await;
            let Ok(recovery) = plugin.store.recovery_effect().await else {
                return;
            };
            let mut provider_started = false;
            let outcome = async {
                let Some(engine) = plugin
                    .store
                    .find_engine(&customer)
                    .await
                    .map_err(store_error)?
                else {
                    return Ok(());
                };
                let engines = plugin.engines()?;
                if !engine.desired_enabled {
                    provider_started = true;
                    engines
                        .suspend(&customer)
                        .await
                        .map_err(|_| unavailable("engine", "suspension pending"))?;
                    return plugin
                        .store
                        .engine_state(&customer, "suspended")
                        .await
                        .map_err(store_error);
                }
                let admin = plugin.engine_password(&customer).await?;
                provider_started = true;
                if engines.provision(&customer, &admin).await.is_err() {
                    plugin
                        .store
                        .engine_state(&customer, "failed")
                        .await
                        .map_err(store_error)?;
                    return Err(unavailable("engine", "provisioning failed"));
                }
                plugin
                    .store
                    .engine_state(&customer, "ready")
                    .await
                    .map_err(store_error)
            }
            .await;
            if outcome.is_err() {
                eprintln!("mail_plugin_engine: reconciliation failed");
            }
            if provider_started && outcome.is_err() {
                eprintln!("mail_plugin_recovery_effect_uncertain");
                return;
            }
            if recovery.finish().await.is_err() {
                eprintln!("mail_plugin_recovery_effect_uncertain");
            }
        });
    }
    async fn mailbox(
        &self,
        claims: &InvocationClaims,
        input: Value,
    ) -> Result<Option<store::Mailbox>> {
        let target: Target = parse(input)?;
        let mailbox = self
            .store
            .mailbox(&claims.project_id, &uuid(&target.resource_id)?)
            .await
            .map_err(store_error)?;
        Ok(mailbox.filter(|m| m.customer_id.is_empty() || m.customer_id == claims.customer_id))
    }
    pub(crate) async fn view_mailbox(
        &self,
        claims: &InvocationClaims,
        input: Value,
    ) -> Result<Value> {
        let m = self.mailbox(claims, input).await?.ok_or_else(missing)?;
        let engines = self.engines()?;
        let (imap, submission) = engines.ports();
        let incoming = self.incoming_state(&claims.customer_id).await?;
        let state = if m.customer_id.is_empty() {
            "migration-required"
        } else if incoming["desiredEnabled"] == false {
            if incoming["state"] == "suspending" {
                "suspending"
            } else {
                "suspended"
            }
        } else {
            &m.state
        };
        Ok(json!({
            "address": format!("{}@{}", m.local_part, m.domain), "name": m.display_name, "state": state, "error": m.error,
            "host": if m.customer_id.is_empty() { None } else { Some(engines.host(&m.customer_id)) }, "imapPort": imap, "smtpPort": submission, "username": format!("{}@{}", m.local_part, m.domain),
            "quotaMb": m.quota_mb, "createdAt": m.created_at, "tls": engines.tls_note(),
        }))
    }
    async fn ready_mailbox(
        &self,
        claims: &InvocationClaims,
        input: Value,
    ) -> Result<(store::Mailbox, String)> {
        let m = self.mailbox(claims, input).await?.ok_or_else(missing)?;
        let engine = self
            .store
            .find_engine(&m.customer_id)
            .await
            .map_err(store_error)?;
        if !engine.is_some_and(|e| e.desired_enabled && e.state == "ready") {
            return Err(PluginError::new(
                ErrorCode::Conflict,
                "Incoming mail is not ready",
            ));
        }
        match &m.account_id {
            Some(account) if m.state == "ready" => {
                let account = account.clone();
                Ok((m, account))
            }
            _ => Err(PluginError::new(
                ErrorCode::Conflict,
                "The mailbox is not ready",
            )),
        }
    }
    pub(crate) async fn mailbox_password(
        &self,
        claims: &InvocationClaims,
        input: Value,
    ) -> Result<Value> {
        let (m, _) = self.ready_mailbox(claims, input).await?;
        let password = self
            .sealer
            .open(&m.id, &m.password_sealed)
            .ok_or_else(|| unavailable("unseal", m.id))?;
        Ok(json!({"value": password}))
    }
    pub(crate) async fn rotate_mailbox(
        &self,
        claims: &InvocationClaims,
        input: Value,
    ) -> Result<Value> {
        let _serial = self.provisioning.lock().await;
        let (m, account) = self.ready_mailbox(claims, input).await?;
        let password = seal::password();
        let client = self
            .existing_client(&m.customer_id)
            .await?
            .ok_or_else(|| unavailable("engine", "not running"))?;
        // The admin can always set it again, so a lost store write is repaired by rotating once more.
        client
            .password(&account, &password)
            .await
            .map_err(|e| unavailable("engine", e))?;
        self.store
            .mailbox_password(&m.id, &self.sealer.seal(&m.id, &password))
            .await
            .map_err(store_error)?;
        Ok(json!({}))
    }
    pub(crate) async fn delete_mailbox(
        &self,
        claims: &InvocationClaims,
        input: Value,
    ) -> Result<Value> {
        let _serial = self.provisioning.lock().await;
        // The Core proved ownership; a mailbox already gone here is simply done.
        let Some(m) = self.mailbox(claims, input).await? else {
            return Ok(json!({}));
        };
        if m.customer_id.is_empty() {
            return Err(PluginError::new(
                ErrorCode::Conflict,
                "Incoming mail requires operator migration",
            ));
        }
        if let Some(account) = &m.account_id {
            let client = self
                .existing_client(&m.customer_id)
                .await?
                .ok_or_else(|| unavailable("engine", "not running"))?;
            client
                .delete_mailbox(account)
                .await
                .map_err(|e| unavailable("engine", e))?;
        }
        self.store
            .delete_mailbox(&m.id)
            .await
            .map_err(store_error)?;
        Ok(json!({}))
    }
    /// DKIM record for a domain that has mailboxes on the customer's engine, if the engine runs.
    pub(crate) async fn dkim(&self, customer: &str, domain: &str) -> Option<(String, String)> {
        let client = self.existing_client(customer).await.ok()??;
        let rows: engine::Rows<engine::Named> = client
            .call(
                "x:Domain/get",
                json!({"ids":null,"properties":["id","name"]}),
            )
            .await
            .ok()?;
        let id = rows.list.into_iter().find(|d| d.name == domain)?.id;
        client.dkim(&id).await.ok().flatten()
    }
}

#[cfg(test)]
mod recovery_tests;
