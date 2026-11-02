//! Transfer locations are encrypted separately from live authority; no invocation token is renewed.
use super::*;
use crate::storage::StorageGateway;
use webdock_plugin_http::storage::StorageBinding;
use webdock_plugin_protocol::storage::{
    StorageAuthorization, StorageDirection, StoragePlan, TransferDescriptor,
};
use webdock_secrets::SecretCipher;

pub struct StorageCaller {
    subject: String,
    session: String,
    reference: AuthorityReference,
    can_write: bool,
    customer: Option<String>,
}
impl StorageCaller {
    pub fn browser(
        principal: &Principal,
        authority: webdock_browser_session::StoredAuthority,
        can_write: bool,
    ) -> Self {
        Self {
            subject: principal.user_id.clone(),
            session: principal.session_id.clone(),
            reference: AuthorityReference::Browser { authority },
            can_write,
            customer: None,
        }
    }
    pub fn workspace(
        principal: &Principal,
        authority: webdock_browser_session::StoredAuthority,
        can_write: bool,
        customer: String,
    ) -> Self {
        Self {
            subject: principal.user_id.clone(),
            session: principal.session_id.clone(),
            reference: AuthorityReference::Browser { authority },
            can_write,
            customer: Some(customer),
        }
    }
    pub fn mcp(actor: &webdock_oauth::McpAuthentication) -> Self {
        Self {
            subject: actor.principal.user_id.clone(),
            session: actor.principal.session_id.clone(),
            reference: AuthorityReference::Mcp {
                authority: actor.authority.clone(),
            },
            can_write: actor.authority.can_write(),
            customer: None,
        }
    }
}
/// Private Core transport context. Never serialize this structure to a public response.
pub struct StorageAccess {
    pub plan: StoragePlan,
    pub binding: StorageBinding,
    pub transport_sha256: String,
    pub plugin_id: String,
    pub generation: u64,
    pub authorization: StorageAuthorization,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PrivateTransfer {
    plan: StoragePlan,
    peer: StoredPeer,
    transport_sha256: String,
    source_invocation: String,
    source_operation: String,
    source_api_hash: Option<String>,
}
// The initial transport authenticates the chain; live enrollment and expiry use its leaf.
// Base64 avoids expanding DER into JSON integer arrays inside the bounded private ledger.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum StoredPeer {
    Unix { uid: u32 },
    MutualTls { leaf: String },
}
impl StoredPeer {
    fn capture(peer: &PeerIdentity) -> Result<Self> {
        match peer {
            PeerIdentity::Unix { uid } => Ok(Self::Unix { uid: *uid }),
            PeerIdentity::MutualTls { certificates } => Ok(Self::MutualTls {
                leaf: {
                    let leaf = certificates
                        .first()
                        .filter(|leaf| !leaf.is_empty() && leaf.len() <= 4096)
                        .ok_or_else(PluginError::denied)?;
                    URL_SAFE_NO_PAD.encode(leaf)
                },
            }),
        }
    }
    fn identity(&self) -> Result<PeerIdentity> {
        match self {
            Self::Unix { uid } => Ok(PeerIdentity::Unix { uid: *uid }),
            Self::MutualTls { leaf } => Ok(PeerIdentity::MutualTls {
                certificates: vec![
                    URL_SAFE_NO_PAD
                        .decode(leaf)
                        .map_err(|_| PluginError::denied())?,
                ],
            }),
        }
    }
}
impl HostService {
    pub fn with_storage_gateway(mut self, gateway: Arc<StorageGateway>) -> Self {
        self.storage_gateway = Some(gateway);
        self
    }
    pub fn storage_gateway(&self) -> Result<&Arc<StorageGateway>> {
        self.storage_gateway
            .as_ref()
            .ok_or_else(|| unsupported("Storage gateway unavailable"))
    }
    fn storage_cipher(&self) -> Result<SecretCipher> {
        let mut key = Sha256::new();
        key.update(b"webdock.native.storage.plan-sealing.v1\0");
        key.update(self.key.to_bytes());
        SecretCipher::new(&hex::encode(key.finalize())).map_err(|_| unavailable())
    }
    pub async fn retain_storage_plan(
        &self,
        handle: &InvocationHandle,
        plan: StoragePlan,
        transport_sha256: String,
    ) -> Result<TransferDescriptor> {
        self.retain_storage_plan_with_api(handle, plan, transport_sha256, None)
            .await
    }
    pub async fn retain_storage_plan_with_api(
        &self,
        handle: &InvocationHandle,
        plan: StoragePlan,
        transport_sha256: String,
        source_api_hash: Option<String>,
    ) -> Result<TransferDescriptor> {
        if source_api_hash
            .as_ref()
            .is_some_and(|h| h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return denied();
        }
        let context = self.authorized(handle, None).await?;
        let (inv, _, _) = self.persisted(&handle.claims).await?;
        let action = context
            .manifest
            .action(&handle.claims.action)
            .ok_or_else(PluginError::denied)?;
        if action["resource"] != "use"
            || action["storage"] != plan.direction.as_str()
            || plan.direction == StorageDirection::Upload && action["intent"] != "write"
            || transport_sha256.len() != 64
            || !transport_sha256.bytes().all(|b| b.is_ascii_hexdigit())
            || inv.row["session_context"]["authority"].is_null()
        {
            return denied();
        }
        plan.validate(
            action["storage"].as_str().ok_or_else(PluginError::denied)?,
            inv.row["session_context"]["resourceID"]
                .as_str()
                .ok_or_else(PluginError::denied)?,
            chrono::Utc::now().timestamp(),
        )?;
        let binding = context
            .binding
            .storage
            .get(&handle.claims.project_id)
            .and_then(|m| m.get(&plan.binding))
            .ok_or_else(PluginError::denied)?
            .clone();
        self.storage_gateway()?.target(&plan, binding)?;
        let id = uuid::Uuid::new_v4().to_string();
        let descriptor = TransferDescriptor::new(&id, &plan)?;
        let expires_at = plan.expires_at;
        let private = PrivateTransfer {
            plan,
            peer: StoredPeer::capture(&handle.peer)?,
            transport_sha256,
            source_invocation: inv.id.clone(),
            source_operation: string(&inv.row, "operation_id")?.into(),
            source_api_hash,
        };
        // The cipher has a 24,520-byte plaintext cap, including the sole TLS leaf and provenance.
        if serde_json::to_vec(&private)
            .map_err(|_| invalid("Storage transfer context too large"))?
            .len()
            > 24520
        {
            return Err(invalid("Storage transfer context too large"));
        }
        let sealed = self
            .storage_cipher()?
            .seal(&private, &format!("storage-transfer:{id}"))
            .map_err(|_| unavailable())?;
        let mut stored = inv.row["session_context"].clone();
        stored["storageTransfer"] =
            json!({"sealed":sealed,"expiresAt":expires_at,"state":"pending"});
        // This row has no signed invocation envelope. Its normal sixty-second expiry is never
        // extended or used as data authority; the sealed plan and TWO live principals are checked.
        let tx = self.controller().await?;
        budget_context(&tx, &stored).await?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "INSERT INTO webdock_auth.plugin_invocation(id,plugin_id,customer_id,project_id,runtime_generation,grant_revision,kind,name,intent,actor_id,session_id,operation_id,capabilities,created_at,expires_at,session_context,request_hash) SELECT $1::uuid,plugin_id,customer_id,project_id,runtime_generation,grant_revision,kind,name,intent,actor_id,session_id,$2,capabilities,clock_timestamp(),clock_timestamp()+interval '59 seconds',$3,request_hash FROM webdock_auth.plugin_invocation WHERE id=$4::uuid AND revoked_at IS NULL AND expires_at>clock_timestamp()",
            [id.clone().into(),format!("storage-{id}").into(),stored.into(),inv.id.into()])).await.map_err(db_error).and_then(|r|if r.rows_affected()==1 {Ok(())}else{denied()})?;
        self.finish(context, handle, None).await?;
        tx.commit().await.map_err(db_error)?;
        Ok(descriptor)
    }
    pub async fn storage_access(
        &self,
        id: &str,
        method: &str,
        caller: &StorageCaller,
    ) -> Result<StorageAccess> {
        if uuid::Uuid::parse_str(id).is_err() {
            return denied();
        }
        let tx = self.controller().await?;
        fence(&tx, false).await?;
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT to_jsonb(v) AS invocation FROM webdock_auth.plugin_invocation v WHERE id=$1::uuid AND revoked_at IS NULL AND kind='action'",[id.into()])).await.map_err(db_error)?.ok_or_else(PluginError::denied)?;
        let row: Value = row.try_get("", "invocation").map_err(db_error)?;
        let transfer = &row["session_context"]["storageTransfer"];
        if !["pending", "consuming"].contains(&transfer["state"].as_str().unwrap_or("")) {
            return denied();
        }
        let private: PrivateTransfer = self
            .storage_cipher()?
            .open(
                transfer["sealed"]
                    .as_str()
                    .ok_or_else(PluginError::denied)?,
                &format!("storage-transfer:{id}"),
            )
            .map_err(|_| PluginError::denied())?;
        let plan = private.plan;
        let project = string(&row, "project_id")?;
        let plugin = string(&row, "plugin_id")?;
        let metadata = self.metadata(&tx, plugin, Some(project)).await?;
        let binding = self.binding(plugin, &metadata).await?;
        let configuration = Self::current_configuration(&metadata, &binding, project)?;
        let action = self.action(&metadata, string(&row, "name")?)?;
        if plan.direction.method() != method
            || caller.subject != row["actor_id"]
            || caller
                .customer
                .as_ref()
                .is_some_and(|customer| row["customer_id"] != *customer)
            || plan.expires_at <= chrono::Utc::now().timestamp()
            || plan.expires_at
                != transfer["expiresAt"]
                    .as_i64()
                    .ok_or_else(PluginError::denied)?
            || row["session_context"]["nativeAdmission"]
                != Self::admission_snapshot(&metadata, &binding, &configuration)?
            || !binding.permits(&private.peer.identity()?)
            || metadata.installation["state"] != "enabled"
            || metadata.runtime["status"] != "ready"
            || metadata.grant["revision"] != row["grant_revision"]
            || metadata.grant["customer_id"] != row["customer_id"]
            || row["runtime_generation"] != binding.generation
            || action["storage"] != plan.direction.as_str()
            || row["intent"] != action["intent"]
            || row["capabilities"] != action["capabilities"]
            || action["resource"] != "use"
            || row["session_context"]["resourceID"] != plan.resource_id
            || row["intent"] == "write" && !caller.can_write
        {
            return denied();
        }
        let original: AuthorityReference =
            serde_json::from_value(row["session_context"]["authority"].clone())
                .map_err(|_| PluginError::denied())?;
        self.storage_subject(
            &tx,
            &row,
            action,
            (
                string(&row, "actor_id")?,
                string(&row, "session_id")?,
                &original,
            ),
            true,
        )
        .await?;
        if let Some(hash) = private.source_api_hash {
            let api = self.db.begin().await.map_err(db_error)?;
            let found: Option<Value> = api
                .query_one_raw(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT webdock_rls.api_token_session($1) AS found",
                    [hash.into()],
                ))
                .await
                .map_err(db_error)?
                .ok_or_else(PluginError::denied)?
                .try_get("", "found")
                .map_err(db_error)?;
            let found = found.ok_or_else(PluginError::denied)?;
            if found["user"] != row["actor_id"]
                || found["session"] != row["session_id"]
                || found["customer"] != row["customer_id"]
                || row["intent"] == "write" && found["write"] != true
            {
                return denied();
            }
            api.commit().await.map_err(db_error)?;
        }
        self.storage_subject(
            &tx,
            &row,
            action,
            (&caller.subject, &caller.session, &caller.reference),
            false,
        )
        .await?;
        let storage = binding
            .storage
            .get(project)
            .and_then(|m| m.get(&plan.binding))
            .ok_or_else(PluginError::denied)?
            .clone();
        self.storage_gateway()?.target(&plan, storage.clone())?;
        tx.commit().await.map_err(db_error)?;
        let authorization = StorageAuthorization {
            version: 1,
            transfer_id: id.into(),
            source_invocation: private.source_invocation,
            plugin_id: plugin.into(),
            generation: binding.generation,
            action: string(&row, "name")?.into(),
            customer_id: string(&row, "customer_id")?.into(),
            project_id: project.into(),
            resource_id: plan.resource_id.clone(),
            resource_epoch: plan.resource_epoch.clone(),
            direction: plan.direction,
            exact_bytes: plan.exact_bytes,
            expected_sha256: plan.expected_sha256.clone(),
            expires_at: plan.expires_at,
        };
        Ok(StorageAccess {
            authorization,
            plan,
            binding: storage,
            transport_sha256: private.transport_sha256,
            plugin_id: plugin.into(),
            generation: binding.generation,
        })
    }
    async fn storage_subject(
        &self,
        policy: &DatabaseTransaction,
        row: &Value,
        action: &Value,
        identity: (&str, &str, &AuthorityReference),
        source: bool,
    ) -> Result<()> {
        let (subject, session, reference) = identity;
        if matches!(reference,AuthorityReference::Mcp{authority} if !authority.registry_scope()) {
            return denied();
        }
        let verifier = self
            .session_authority
            .as_ref()
            .ok_or_else(PluginError::denied)?;
        let write = row["intent"] == "write";
        let (auth, principal) = verifier
            .begin_principal(subject, session, reference, write)
            .await?;
        if source && row["session_context"]["customerScope"] != json!(principal.customer_scope()) {
            return denied();
        }
        let business = self
            .signer
            .begin(&self.db, &principal)
            .await
            .map_err(|_| PluginError::denied())?;
        timeouts(&business).await?;
        if Self::project(&business, string(row, "project_id")?, write).await?
            != string(row, "customer_id")?
        {
            return denied();
        }
        self_service::gate(
            policy,
            principal.is_operator(),
            action,
            string(row, "plugin_id")?,
            string(row, "customer_id")?,
            string(row, "project_id")?,
            row["session_context"]["resourceID"].as_str(),
        )
        .await?;
        verifier
            .verify(&auth, subject, session, reference, write)
            .await?;
        business.commit().await.map_err(db_error)?;
        auth.commit().await.map_err(db_error)
    }
    /// One attempt only. Cancellation/uncertain provider outcome never turns the ticket reusable.
    pub async fn claim_storage_upload(&self, id: &str, caller: &StorageCaller) -> Result<()> {
        self.storage_access(id, "PUT", caller).await?;
        let tx = self.controller().await?;
        let result=tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_invocation SET session_context=jsonb_set(session_context,'{storageTransfer,state}','\"consuming\"') WHERE id=$1::uuid AND revoked_at IS NULL AND session_context#>>'{storageTransfer,state}'='pending'",[id.into()])).await.map_err(db_error)?;
        if result.rows_affected() != 1 {
            return Err(conflict("Storage transfer already attempted"));
        }
        tx.commit().await.map_err(db_error)
    }
    pub async fn finish_storage_upload(&self, id: &str, caller: &StorageCaller) -> Result<()> {
        self.storage_access(id, "PUT", caller).await?;
        let tx = self.controller().await?;
        let result=tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_invocation SET revoked_at=clock_timestamp(),session_context=jsonb_set(session_context,'{storageTransfer,state}','\"complete\"') WHERE id=$1::uuid AND revoked_at IS NULL AND session_context#>>'{storageTransfer,state}'='consuming'",[id.into()])).await.map_err(db_error)?;
        if result.rows_affected() != 1 {
            return denied();
        }
        tx.commit().await.map_err(db_error)
    }
}

// Measure PostgreSQL's complete jsonb representation, including whitespace/number expansion.
// Keep this budget in sync with migration 0049; never turn size failures into generic SQL 503s.
async fn budget_context(tx: &DatabaseTransaction, stored: &Value) -> Result<()> {
    let row = tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT octet_length($1::jsonb::text)<=65536*128+65536 AND octet_length(($1::jsonb #- '{nativeAdmission,configuration}')::text)<=65536 AND octet_length((($1::jsonb-'storageTransfer') #- '{nativeAdmission,configuration}')::text)<=16384 AS fits",
        [stored.clone().into()])).await.map_err(db_error)?.ok_or_else(unavailable)?;
    if !row.try_get::<bool>("", "fits").map_err(db_error)? {
        return Err(invalid("Storage transfer context too large"));
    }
    Ok(())
}

impl HostService {
    /// Fixed archives source revalidation, reached only over the pinned runtime callback.
    pub async fn export_storage_scope(
        &self,
        peer: PeerIdentity,
        authorization: &StorageAuthorization,
    ) -> Result<(super::InvocationScope, sea_orm::DatabaseTransaction)> {
        if authorization.plugin_id != "archives" || authorization.action != "archives.download" {
            return denied();
        }
        let binding = self
            .bindings
            .runtime("archives")
            .await?
            .ok_or_else(PluginError::denied)?;
        if binding.generation != authorization.generation || !binding.permits(&peer) {
            return denied();
        }
        let tx = self.controller().await?;
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT to_jsonb(v) AS invocation FROM webdock_auth.plugin_invocation v WHERE id=$1::uuid AND plugin_id='archives' AND revoked_at IS NULL",[authorization.transfer_id.clone().into()])).await.map_err(db_error)?.ok_or_else(PluginError::denied)?;
        let row: Value = row.try_get("", "invocation").map_err(db_error)?;
        let reference: AuthorityReference =
            serde_json::from_value(row["session_context"]["authority"].clone())
                .map_err(|_| PluginError::denied())?;
        let caller = StorageCaller {
            subject: string(&row, "actor_id")?.into(),
            session: string(&row, "session_id")?.into(),
            reference: reference.clone(),
            can_write: true,
            customer: Some(authorization.customer_id.clone()),
        };
        tx.commit().await.map_err(db_error)?;
        let access = self
            .storage_access(&authorization.transfer_id, "GET", &caller)
            .await?;
        if serde_json::to_value(&access.authorization).map_err(|_| unavailable())?
            != serde_json::to_value(authorization).map_err(|_| unavailable())?
        {
            return denied();
        }
        let verifier = self
            .session_authority
            .as_ref()
            .ok_or_else(PluginError::denied)?;
        let (auth, principal) = verifier
            .begin_principal(&caller.subject, &caller.session, &reference, false)
            .await?;
        let business = self
            .signer
            .begin(&self.db, &principal)
            .await
            .map_err(|_| PluginError::denied())?;
        auth.commit().await.map_err(db_error)?;
        Ok((
            super::InvocationScope {
                plugin_id: "archives".into(),
                customer_id: authorization.customer_id.clone(),
                project_id: authorization.project_id.clone(),
                operation_id: authorization.source_invocation.clone(),
                subject: caller.subject,
                session_id: caller.session,
                generation: authorization.generation,
                grant_revision: integer(&row, "grant_revision")?,
            },
            business,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_maximum_plan_seals_with_large_tls_leaf_and_bounded_outer_authority() {
        let mut plan: StoragePlan=serde_json::from_value(json!({"version":1,"binding":"objects","direction":"download","resourceId":"owned","resourceEpoch":"active-1","url":format!("https://storage.invalid/private/key?X-Amz-Security-Token={}","a".repeat(8000)),"headers":{},"expiresAt":1300,"exactBytes":3,"expectedSha256":"a".repeat(64),"attachment":{"filename":"file.bin","contentType":"application/octet-stream"}})).unwrap();
        for i in 0..3 {
            plan.headers.insert(format!("test-{i}"), "a".repeat(1000));
        }
        let size = serde_json::to_vec(&plan).unwrap().len();
        plan.headers.insert("padding".into(), String::new());
        let overhead = serde_json::to_vec(&plan).unwrap().len() - size;
        plan.headers.insert(
            "padding".into(),
            "a".repeat(webdock_plugin_protocol::storage::MAX_STORAGE_PLAN_BYTES - size - overhead),
        );
        plan.validate("download", "owned", 1000).unwrap();
        assert_eq!(serde_json::to_vec(&plan).unwrap().len(), 12 * 1024);
        let peer = StoredPeer::capture(&PeerIdentity::MutualTls {
            certificates: vec![vec![1; 2048], vec![2; 8192]],
        })
        .unwrap();
        assert!(
            serde_json::to_vec(&peer).unwrap().len() < 2800,
            "unneeded certificate chain must be discarded"
        );
        let private = PrivateTransfer {
            plan,
            peer,
            transport_sha256: "c".repeat(64),
            source_invocation: uuid::Uuid::new_v4().to_string(),
            source_operation: "x".repeat(128),
            source_api_hash: Some("d".repeat(64)),
        };
        let cipher = SecretCipher::new(&"test".repeat(16)).unwrap();
        let sealed = cipher.seal(&private, "storage-transfer:boundary").unwrap();
        assert!(
            sealed.len() > 16384,
            "regression must exceed ordinary ledger size"
        );
        let context = json!({"authority":{"kind":"mcp","opaqueContext":"a".repeat(15000)},"storageTransfer":{"sealed":sealed,"expiresAt":1300,"state":"pending"}});
        assert!(serde_json::to_vec(&context).unwrap().len() < 65536);
        let opened: PrivateTransfer = cipher
            .open(
                context["storageTransfer"]["sealed"].as_str().unwrap(),
                "storage-transfer:boundary",
            )
            .unwrap();
        assert_eq!(opened.plan.exact_bytes, 3);
        assert!(!context.to_string().contains("storage.invalid"));
        assert!(!context.to_string().contains("Security-Token"));
        assert!(
            StoredPeer::capture(&PeerIdentity::MutualTls {
                certificates: vec![vec![0; 4097]]
            })
            .is_err()
        );
    }
}
