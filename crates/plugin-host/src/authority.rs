use crate::session_authority::AuthorityReference;
use crate::{
    bindings::{RuntimeBinding, RuntimeBindings},
    conflict, db_error, invalid,
    scope::HostScopes,
    slug, unavailable, unsupported,
};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer, SigningKey};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, Statement,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use spitzli_identity::{Principal, RlsSigner};
use spitzli_persistence::IdAllocator;
use spitzli_plugin_protocol::{
    InvocationClaims, Manifest, PluginError, Result, validate_value, verify_invocation,
};
use spitzli_plugin_runtime::{InvocationAuthority, PeerIdentity};
use std::sync::Arc;

pub const MAIL_ACTION_INPUT_BYTES: usize = 128 * 1024;
// These reads enumerate beyond an already attached database and retain the
// workspace-administrator requirement of the existing Connections inventory.
pub(crate) fn neon_management(write: bool, action: &str) -> bool {
    write || matches!(action, "databases.discover" | "databases.branches")
}

#[path = "database_orders.rs"]
mod database_orders;
pub(crate) fn invocation_input_limit(plugin: &str, action: &str, job: bool) -> usize {
    if !job && plugin == "mail" && action == "mail.send" {
        MAIL_ACTION_INPUT_BYTES
    } else {
        32768
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdmissionInput {
    #[serde(rename = "pluginID")]
    pub plugin_id: String,
    #[serde(rename = "projectID")]
    pub project_id: String,
    pub action: String,
    pub input: Value,
    pub idempotency_key: String,
}
pub struct AdmittedInvocation {
    pub claims: InvocationClaims,
    pub token: String,
    pub configuration: Value,
    pub input: Value,
    /// Already authorized retained public result; callers must return it without runtime dispatch.
    pub replay_result: Option<Value>,
}
pub struct InvocationHandle {
    pub(crate) peer: PeerIdentity,
    pub(crate) claims: InvocationClaims,
}
impl InvocationHandle {
    pub fn claims(&self) -> &InvocationClaims {
        &self.claims
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvocationScope {
    #[serde(rename = "pluginID")]
    pub plugin_id: String,
    #[serde(rename = "customerID")]
    pub customer_id: String,
    #[serde(rename = "projectID")]
    pub project_id: String,
    #[serde(rename = "operationID")]
    pub operation_id: String,
    pub subject: String,
    #[serde(rename = "sessionID")]
    pub session_id: String,
    pub generation: u64,
    pub grant_revision: i64,
}
/// Product adapter owns live app/resource binding checks and validates its typed result contract.
/// Host supplies only a freshly checked invocation scope and revalidates before returning results.
#[async_trait]
pub trait AppsAdapter: Send + Sync {
    async fn request(&self, scope: &InvocationScope, input: Value) -> Result<Value>;
}
pub struct HostService {
    installation: Option<crate::installation::InstallationConfig>,
    native_mail_control: bool,
    new_database_orders_enabled: bool,
    storage_gateway: Option<Arc<crate::storage::StorageGateway>>,
    pub(crate) db: DatabaseConnection,
    worker: DatabaseConnection,
    signer: RlsSigner,
    scopes: HostScopes,
    pub(crate) ids: Arc<IdAllocator>,
    bindings: Arc<dyn RuntimeBindings>,
    key: SigningKey,
    apps: Option<Arc<dyn AppsAdapter>>,
    pub(crate) named_http: Option<Arc<dyn crate::http::NamedHttpAdapter>>,
    session_authority: Option<crate::session_authority::SessionAuthority>,
}
pub(crate) struct PersistedInvocation {
    pub id: String,
    pub actor: String,
    pub session: String,
    pub row: Value,
}
pub(crate) struct Metadata {
    pub release: Value,
    pub installation: Value,
    pub runtime: Value,
    pub configuration: Value,
    pub grant: Value,
    pub manifest: Manifest,
}
pub(crate) struct Authorized {
    pub tx: DatabaseTransaction,
    auth_tx: Option<DatabaseTransaction>,
    authority_ref: Option<AuthorityReference>,
    intent_write: bool,
    pub(crate) principal: Option<Principal>,
    pub(crate) invocation: Value,
    binding_hash: String,
    pub scope: InvocationScope,
    pub binding: RuntimeBinding,
    pub manifest: Manifest,
}
fn denied<T>() -> Result<T> {
    Err(PluginError::denied())
}
fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key].as_str().ok_or_else(unavailable)
}
fn integer(value: &Value, key: &str) -> Result<i64> {
    value[key].as_i64().ok_or_else(unavailable)
}
pub(crate) fn digest(bytes: impl AsRef<[u8]>) -> String {
    hex::encode(Sha256::digest(bytes.as_ref()))
}
impl HostService {
    pub async fn new(
        db: DatabaseConnection,
        worker: DatabaseConnection,
        secret: String,
        ids: Arc<IdAllocator>,
        bindings: Arc<dyn RuntimeBindings>,
        key: SigningKey,
    ) -> Result<Self> {
        for (connection, expected) in [
            (&db, "webdock_auth_runtime"),
            (&worker, "webdock_worker_runtime"),
        ] {
            spitzli_persistence::connection::verify_runtime_role(connection, expected)
                .await
                .map_err(|_| invalid("Restricted plugin database roles required"))?;
        }
        let signer = RlsSigner::new(secret.clone(), "webdock_auth_runtime")
            .map_err(|_| invalid("Invalid RLS configuration"))?;
        Ok(Self {
            db,
            worker,
            signer,
            scopes: HostScopes::new(secret)?,
            ids,
            bindings,
            key,
            apps: None,
            named_http: None,
            session_authority: None,
            installation: None,
            native_mail_control: false,
            new_database_orders_enabled: false,
            storage_gateway: None,
        })
    }
    pub async fn with_session_authority(
        mut self,
        auth: DatabaseConnection,
        identity: Arc<spitzli_identity::IdentityService>,
        browser: Option<Arc<spitzli_browser_session::BrowserSessions>>,
    ) -> Result<Self> {
        self.session_authority =
            Some(crate::session_authority::SessionAuthority::new(auth, identity, browser).await?);
        Ok(self)
    }
    pub fn with_mcp_authority(mut self, oauth: Arc<spitzli_oauth::OAuthService>) -> Result<Self> {
        self.session_authority
            .as_mut()
            .ok_or_else(|| invalid("Session verifier is required"))?
            .mcp = Some(oauth);
        Ok(self)
    }
    pub fn with_native_mail_control(mut self, available: bool) -> Self {
        self.native_mail_control = available;
        self
    }
    pub fn with_new_database_orders_enabled(mut self, enabled: bool) -> Self {
        self.new_database_orders_enabled = enabled;
        self
    }
    fn blocked_database_creates(&self, manifest: &Manifest) -> Vec<String> {
        ["databases.create", "instances.create"]
            .into_iter()
            .filter(|action| {
                manifest.action(action).is_some()
                    && !database_orders::create_allowed(
                        self.new_database_orders_enabled,
                        manifest.id(),
                        action,
                        false,
                    )
            })
            .map(str::to_owned)
            .collect()
    }
    pub fn with_named_http(mut self, adapter: Arc<dyn crate::http::NamedHttpAdapter>) -> Self {
        self.named_http = Some(adapter);
        self
    }
    pub fn with_apps(mut self, apps: Arc<dyn AppsAdapter>) -> Self {
        self.apps = Some(apps);
        self
    }
    pub fn runtime_authority(
        self: &Arc<Self>,
        plugin_id: String,
        generation: u64,
    ) -> Result<RuntimeAuthority> {
        if !slug(&plugin_id) || generation == 0 {
            return Err(invalid("Invalid runtime identity"));
        }
        Ok(RuntimeAuthority {
            host: self.clone(),
            plugin_id,
            generation,
        })
    }
    pub(crate) async fn controller(&self) -> Result<DatabaseTransaction> {
        let tx = self.worker.begin().await.map_err(db_error)?;
        self.scopes.controller(&tx).await?;
        timeouts(&tx).await?;
        Ok(tx)
    }
    async fn metadata(
        &self,
        tx: &DatabaseTransaction,
        plugin: &str,
        project: Option<&str>,
    ) -> Result<Metadata> {
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT to_jsonb(p) AS installation,to_jsonb(r) AS runtime,to_jsonb(c) AS configuration,to_jsonb(l) AS release,to_jsonb(g) AS grant FROM webdock_auth.plugin_installation p JOIN webdock_auth.plugin_runtime r ON r.plugin_id=p.id AND r.generation=p.current_generation JOIN webdock_auth.plugin_configuration c ON c.plugin_id=p.id JOIN webdock_auth.plugin_release l ON l.plugin_id=p.id AND l.version=p.version LEFT JOIN webdock_auth.plugin_grant g ON g.plugin_id=p.id AND g.project_id=$2 WHERE p.id=$1 AND p.kind='external'",[plugin.into(),project.into()])).await.map_err(db_error)?.ok_or_else(PluginError::denied)?;
        let release: Value = row.try_get("", "release").map_err(db_error)?;
        let raw = string(&release, "manifest_bytes")?;
        if digest(raw) != string(&release, "manifest_sha256")? {
            return Err(unavailable());
        }
        let manifest = Manifest::parse(raw.as_bytes())?;
        let installation: Value = row.try_get("", "installation").map_err(db_error)?;
        let runtime: Value = row.try_get("", "runtime").map_err(db_error)?;
        let configuration: Value = row.try_get("", "configuration").map_err(db_error)?;
        if manifest.id() != plugin
            || manifest.version() != string(&installation, "version")?
            || manifest.as_json()["image"] != release["image"]
            || runtime["image"] != release["image"]
            || runtime["version"] != installation["version"]
        {
            return Err(unavailable());
        }
        Ok(Metadata {
            release,
            installation,
            runtime,
            configuration,
            grant: row
                .try_get::<Option<Value>>("", "grant")
                .map_err(db_error)?
                .unwrap_or(Value::Null),
            manifest,
        })
    }
    async fn binding(&self, plugin: &str, metadata: &Metadata) -> Result<RuntimeBinding> {
        let binding = self
            .bindings
            .runtime(plugin)
            .await?
            .ok_or_else(|| unsupported("Plugin runtime is not configured"))?;
        self.checked_binding(metadata, binding)
    }
    fn checked_binding(
        &self,
        metadata: &Metadata,
        binding: RuntimeBinding,
    ) -> Result<RuntimeBinding> {
        let mut services = vec![("data", 1), ("http", 1), ("jobs", 1)];
        if self.native_mail_control && metadata.manifest.id() == "mail" {
            services.push(("native-mail-control", 1));
        }
        if self.storage_gateway.is_some() {
            services.push(("storage", 1));
        }
        if self.apps.is_some() {
            services.push(("apps", 1));
        }
        metadata.manifest.require_host_services(&services)?;
        if binding.generation == 0
            || binding.generation > 9_007_199_254_740_991
            || integer(&metadata.installation, "current_generation")? != binding.generation as i64
            || integer(&metadata.runtime, "generation")? != binding.generation as i64
            || metadata.runtime["namespace"] != binding.namespace
            || metadata.runtime["service_name"] != binding.service
            || metadata.runtime["credential_hash"] != binding.credential_sha256
            || metadata.installation["runtime"] != "ready"
            || ![json!("ready"), json!("draining")].contains(&metadata.runtime["status"])
            || metadata.runtime["configuration_revision"] != metadata.configuration["revision"]
        {
            return denied();
        }
        Ok(binding)
    }
    fn action<'a>(&self, metadata: &'a Metadata, name: &str) -> Result<&'a Value> {
        let action = metadata
            .manifest
            .action(name)
            .ok_or_else(PluginError::denied)?;
        // Website actions require a separately ported live website/SSO authority adapter.
        if action.get("httpBinding").is_some() {
            return Err(unsupported("Website HTTP authority is unavailable"));
        }
        for cap in action["capabilities"].as_array().ok_or_else(unavailable)? {
            let capability = cap.as_str().ok_or_else(unavailable)?;
            if ![
                "plugin.data.read",
                "plugin.data.write",
                "plugin.http.read",
                "plugin.http.request",
                "plugin.jobs.enqueue",
                "plugin.jobs.read",
                "plugin.jobs.cancel",
            ]
            .contains(&capability)
                && !(self.storage_gateway.is_some()
                    && ["plugin.storage.read", "plugin.storage.write"].contains(&capability))
                && !(self.apps.is_some()
                    && [
                        "plugin.apps.read",
                        "plugin.apps.manage",
                        "plugin.apps.provision",
                    ]
                    .contains(&capability))
            {
                return Err(unsupported("Requested host capability is unavailable"));
            }
            if !metadata.configuration["approved_capabilities"]
                .as_array()
                .is_some_and(|v| v.contains(cap))
                || !metadata.grant["capabilities"]
                    .as_array()
                    .is_some_and(|v| v.contains(cap))
            {
                return denied();
            }
        }
        if metadata.grant.is_null() || !metadata.grant["revoked_at"].is_null() {
            return denied();
        }
        Ok(action)
    }
    async fn project(tx: &DatabaseTransaction, project: &str, write: bool) -> Result<String> {
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT p.customer_id,webdock_rls.customer_allowed(p.customer_id,$2) AS allowed FROM webdock_admin.projects p JOIN webdock_admin.customers c ON c.id=p.customer_id WHERE p.id=$1 AND p.status='active' AND c.status='active' AND (webdock_rls.actor()->'preview'='null'::jsonb OR webdock_rls.acting_customer()=p.customer_id) AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=p.id AND d.status<>'deleted')",[project.into(),write.into()])).await.map_err(db_error)?.ok_or_else(PluginError::denied)?;
        if !row.try_get::<bool>("", "allowed").map_err(db_error)? {
            return denied();
        }
        row.try_get("", "customer_id").map_err(db_error)
    }
    pub async fn admit(
        &self,
        actor: &Principal,
        input: AdmissionInput,
    ) -> Result<AdmittedInvocation> {
        self.admit_with_write_ceiling(actor, input, true).await
    }
    pub async fn admit_with_write_ceiling(
        &self,
        actor: &Principal,
        input: AdmissionInput,
        can_write: bool,
    ) -> Result<AdmittedInvocation> {
        self.admit_authority(actor, input, can_write, None).await
    }
    pub async fn admit_with_authority(
        &self,
        actor: &Principal,
        input: AdmissionInput,
        can_write: bool,
        authority: &spitzli_browser_session::StoredAuthority,
    ) -> Result<AdmittedInvocation> {
        self.admit_authority(
            actor,
            input,
            can_write,
            Some(&AuthorityReference::Browser {
                authority: authority.clone(),
            }),
        )
        .await
    }
    pub async fn admit_with_mcp_authority(
        &self,
        actor: &Principal,
        input: AdmissionInput,
        can_write: bool,
        authority: &spitzli_oauth::McpAuthority,
    ) -> Result<AdmittedInvocation> {
        self.admit_authority(
            actor,
            input,
            can_write,
            Some(&AuthorityReference::Mcp {
                authority: authority.clone(),
            }),
        )
        .await
    }
    async fn admit_authority(
        &self,
        actor: &Principal,
        input: AdmissionInput,
        can_write: bool,
        authority: Option<&AuthorityReference>,
    ) -> Result<AdmittedInvocation> {
        let mut input = input;
        input.input = crate::data::legacy_json_value(&input.input)?;
        if !slug(&input.plugin_id)
            || input.project_id.is_empty()
            || input.project_id.len() > 20
            || !input.project_id.bytes().all(|c| c.is_ascii_digit())
            || input.action.is_empty()
            || input.action.encode_utf16().count() > 64
            || input.idempotency_key.is_empty()
            || input.idempotency_key.encode_utf16().count() > 128
            || crate::data::javascript_json(&input.input, false)?.len()
                > invocation_input_limit(&input.plugin_id, &input.action, false)
        {
            return Err(invalid("Invalid plugin admission"));
        }
        // Discovery owns no locks while canonical session authority is acquired.
        let discovery = self.controller().await?;
        let discovered = self
            .metadata(&discovery, &input.plugin_id, Some(&input.project_id))
            .await?;
        let discovered_write = discovered
            .manifest
            .action(&input.action)
            .ok_or_else(PluginError::denied)?["intent"]
            == "write";
        discovery.commit().await.map_err(db_error)?;
        let auth_tx = if let Some(authority) = authority {
            Some(
                self.session_authority
                    .as_ref()
                    .ok_or_else(PluginError::denied)?
                    .begin(
                        &actor.user_id,
                        &actor.session_id,
                        authority,
                        discovered_write,
                    )
                    .await?,
            )
        } else {
            None
        };
        let session = self
            .signer
            .begin(&self.db, actor)
            .await
            .map_err(|_| PluginError::denied())?;
        timeouts(&session).await?;
        let tx = self.controller().await?;
        fence(&tx, false).await?;
        self_service::ensure_grant(&tx, &input.plugin_id, &input.project_id).await?;
        let metadata = self
            .metadata(&tx, &input.plugin_id, Some(&input.project_id))
            .await?;
        let binding = self.binding(&input.plugin_id, &metadata).await?;
        if metadata.installation["state"] != "enabled" || metadata.runtime["status"] != "ready" {
            return denied();
        }
        let action = self.action(&metadata, &input.action)?;
        let write = action["intent"] == "write";
        if write != discovered_write {
            return denied();
        }
        if write && !can_write {
            return denied();
        }
        let customer = Self::project(&session, &input.project_id, write).await?;
        if input.plugin_id == "neon" {
            if !matches!(authority, Some(AuthorityReference::Browser { .. })) {
                return denied();
            }
            Self::neon_scope(&session, &customer, write, &input.action).await?;
        }
        if metadata.grant["customer_id"] != customer {
            return denied();
        }
        let operator: bool = session
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT webdock_rls.operator() AS operator",
            ))
            .await
            .map_err(db_error)?
            .ok_or_else(PluginError::denied)?
            .try_get("", "operator")
            .map_err(db_error)?;
        if actor.customer_scope().is_some() {
            let scope: Option<String> = session
                .query_one_raw(Statement::from_string(
                    DbBackend::Postgres,
                    "SELECT webdock_rls.customer_scope() AS scope",
                ))
                .await
                .map_err(db_error)?
                .ok_or_else(PluginError::denied)?
                .try_get("", "scope")
                .map_err(db_error)?;
            if scope.as_deref() != actor.customer_scope() {
                return denied();
            }
        }
        let operation = digest(
            serde_json::to_vec(&json!([
                actor.user_id,
                input.plugin_id,
                input.project_id,
                input.action,
                input.idempotency_key
            ]))
            .map_err(|_| invalid("Invalid operation"))?,
        );
        let request_hash = digest(crate::data::javascript_json(&input.input, false)?);
        let resource = self_service::resource_input(&input.input).map(str::to_owned);
        let detach_replay = if matches!(authority, Some(AuthorityReference::Browser { .. })) {
            self_service::neon_detach_replay(
                &tx,
                &input.plugin_id,
                &input.action,
                &customer,
                &input.project_id,
                (&actor.user_id, &operation),
                &request_hash,
            )
            .await?
        } else {
            None
        };
        let mut gated_action = action.clone();
        if detach_replay.is_some() {
            gated_action["resource"] = Value::Null;
        }
        self_service::gate(
            &tx,
            operator,
            &gated_action,
            &input.plugin_id,
            &customer,
            &input.project_id,
            resource.as_deref(),
        )
        .await?;
        validate_value(action.get("input"), &input.input)?;
        let mut configuration = metadata.configuration["configuration"]
            .as_object()
            .cloned()
            .ok_or_else(unavailable)?;
        if let Some(overrides) = binding.public_configuration.get(&input.project_id) {
            configuration.extend(
                overrides
                    .as_object()
                    .ok_or_else(|| invalid("Invalid project configuration"))?
                    .clone(),
            );
        }
        let configuration = crate::data::legacy_json_value(&Value::Object(configuration))?;
        validate_value(
            Some(&metadata.manifest.as_json()["configuration"]),
            &configuration,
        )?;
        tx.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
            [operation.clone().into()],
        ))
        .await
        .map_err(db_error)?;
        if let Some(prior)=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT request_hash FROM webdock_auth.plugin_invocation WHERE plugin_id=$1 AND project_id=$2 AND actor_id=$3 AND operation_id=$4 LIMIT 1",[input.plugin_id.clone().into(),input.project_id.clone().into(),actor.user_id.clone().into(),operation.clone().into()])).await.map_err(db_error)? && prior.try_get::<Option<String>>("","request_hash").map_err(db_error)?.as_deref()!=Some(&request_hash){return Err(conflict("Idempotency key reused with different input"));}
        let now = chrono::Utc::now().timestamp();
        let claims = InvocationClaims {
            job: None,
            v: 1,
            id: uuid::Uuid::new_v4().to_string(),
            plugin_id: input.plugin_id.clone(),
            generation: binding.generation,
            action: input.action,
            project_id: input.project_id.clone(),
            customer_id: customer.clone(),
            iat: now,
            exp: now + 60,
        };
        let snapshot = Self::admission_snapshot(&metadata, &binding, &configuration)?;
        let mut stored_scope = json!({"kind":"session","authProvider":"better-auth","subject":actor.user_id,"sessionID":actor.session_id,"nativeAdmission":snapshot,"customerScope":actor.customer_scope(),"authority":authority,"operator":operator,"resourceID":resource,"resourceAction":{"effect":action["resource"],"offering":action["offering"]}});
        // Persist only this explicit nonsecret tuple so SQL can bind atomic writes to admission.
        if claims.plugin_id == "neon" {
            if claims.action == "databases.attach" {
                stored_scope["neonAttachmentInput"] = json!({"name":input.input["name"],"neon_project_id":input.input["neon_project_id"],"branch_id":input.input["branch_id"],"database_name":input.input["database_name"],"role_name":input.input["role_name"],"endpoint_id":input.input["endpoint_id"]});
            } else if claims.action == "databases.detach" {
                stored_scope["neonAttachmentInput"] =
                    json!({"resource_id":input.input["resource_id"]});
            }
        }
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.plugin_invocation(id,plugin_id,customer_id,project_id,runtime_generation,grant_revision,kind,name,intent,actor_id,session_id,operation_id,capabilities,created_at,expires_at,session_context,request_hash) VALUES($1::uuid,$2,$3,$4,$5,$6,'action',$7,$8,$9,$10,$11,$12,to_timestamp($13)-interval '60 seconds',to_timestamp($13),$14,$15)",[claims.id.clone().into(),input.plugin_id.into(),customer.into(),input.project_id.clone().into(),(claims.generation as i64).into(),integer(&metadata.grant,"revision")?.into(),claims.action.clone().into(),string(action,"intent")?.into(),actor.user_id.clone().into(),actor.session_id.clone().into(),operation.into(),action["capabilities"].clone().into(),claims.exp.into(),stored_scope.into(),request_hash.into()])).await.map_err(db_error)?;
        if claims.plugin_id == "neon" && claims.action == "databases.attach" {
            tx.query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.reclaim_neon_attachment_reservation($1::uuid)",
                [claims.id.clone().into()],
            ))
            .await
            .map_err(db_error)?;
        }
        let replay_result = match detach_replay {
            Some(result) => Some(result),
            None => self_service::reserve(&tx, action, &claims.plugin_id, &claims.id).await?,
        };
        // Retained public receipts may replay; a fresh reservation rolls back before dispatch.
        if !database_orders::create_allowed(
            self.new_database_orders_enabled,
            &claims.plugin_id,
            &claims.action,
            replay_result.is_some(),
        ) {
            return Err(conflict(
                "New shared and self-hosted PostgreSQL orders are retired. Use your own Neon account for a new database; existing databases remain available.",
            ));
        }
        Self::project(&session, &input.project_id, write).await?;
        if let (Some(auth_tx), Some(authority)) = (&auth_tx, authority) {
            self.session_authority
                .as_ref()
                .ok_or_else(PluginError::denied)?
                .verify(auth_tx, &actor.user_id, &actor.session_id, authority, write)
                .await?;
        }
        tx.commit().await.map_err(db_error)?;
        session.commit().await.map_err(db_error)?;
        if let Some(auth_tx) = auth_tx {
            auth_tx.commit().await.map_err(db_error)?;
        }
        let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).map_err(|_| unavailable())?);
        let token = format!(
            "{body}.{}",
            URL_SAFE_NO_PAD.encode(self.key.sign(body.as_bytes()).to_bytes())
        );
        Ok(AdmittedInvocation {
            claims,
            token,
            configuration,
            input: input.input,
            replay_result,
        })
    }
    pub async fn authenticate(&self, peer: PeerIdentity, token: &str) -> Result<InvocationHandle> {
        if token.len() > 16384 {
            return denied();
        }
        let body = token.split_once('.').ok_or_else(PluginError::denied)?.0;
        let hint: InvocationClaims = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(body)
                .map_err(|_| PluginError::denied())?,
        )
        .map_err(|_| PluginError::denied())?;
        let claims = verify_invocation(
            token,
            &self.key.verifying_key(),
            &hint.plugin_id,
            hint.generation,
            chrono::Utc::now().timestamp(),
        )?;
        let handle = InvocationHandle { peer, claims };
        self.revalidate(&handle).await?;
        Ok(handle)
    }
    async fn persisted(
        &self,
        claims: &InvocationClaims,
    ) -> Result<(PersistedInvocation, Metadata, RuntimeBinding)> {
        if claims.job.is_some() {
            return self.persisted_job(claims).await;
        }
        let now = chrono::Utc::now().timestamp();
        if claims.v != 1
            || claims.iat > now
            || claims.exp <= now
            || !(1..=60).contains(&(claims.exp.saturating_sub(claims.iat)))
        {
            return denied();
        }
        let tx = self.controller().await?;
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT to_jsonb(v) AS invocation FROM webdock_auth.plugin_invocation v WHERE id=$1::uuid AND revoked_at IS NULL AND created_at<=statement_timestamp() AND expires_at>statement_timestamp() AND expires_at<=statement_timestamp()+interval '60 seconds'",[claims.id.clone().into()])).await.map_err(db_error)?.ok_or_else(PluginError::denied)?;
        let row: Value = row.try_get("", "invocation").map_err(db_error)?;
        let actor = string(&row, "actor_id")?.to_owned();
        let session = string(&row, "session_id")?.to_owned();
        if claims.job.is_some()
            || row["session_context"].get("storageTransfer").is_some()
            || row["kind"] != "action"
            || row["plugin_id"] != claims.plugin_id
            || row["project_id"] != claims.project_id
            || row["customer_id"] != claims.customer_id
            || row["name"] != claims.action
            || integer(&row, "runtime_generation")? != claims.generation as i64
            || row["session_context"]["kind"] != "session"
            || row["session_context"]["subject"] != actor
            || row["session_context"]["sessionID"] != session
            || row["session_context"]
                .get("authProvider")
                .is_some_and(|v| v != "better-auth")
        {
            return denied();
        }
        if chrono::DateTime::parse_from_rfc3339(string(&row, "expires_at")?)
            .map_err(|_| unavailable())?
            .timestamp()
            != claims.exp
        {
            return denied();
        }
        let metadata = self
            .metadata(&tx, &claims.plugin_id, Some(&claims.project_id))
            .await?;
        let binding = self.binding(&claims.plugin_id, &metadata).await?;
        let configuration = Self::current_configuration(&metadata, &binding, &claims.project_id)?;
        if row["session_context"]["nativeAdmission"]
            != Self::admission_snapshot(&metadata, &binding, &configuration)?
        {
            return denied();
        }
        if ![json!("enabled"), json!("draining")].contains(&metadata.installation["state"])
            || metadata.grant["customer_id"] != row["customer_id"]
            || metadata.grant["revision"] != row["grant_revision"]
        {
            return denied();
        }
        let action = self.action(&metadata, &claims.action)?;
        if action["intent"] != row["intent"] || action["capabilities"] != row["capabilities"] {
            return denied();
        }
        let replay = self_service::neon_detach_replay(
            &tx,
            &claims.plugin_id,
            &claims.action,
            string(&row, "customer_id")?,
            &claims.project_id,
            (&actor, string(&row, "operation_id")?),
            string(&row, "request_hash")?,
        )
        .await?;
        let mut gated_action = action.clone();
        if replay.is_some() {
            gated_action["resource"] = Value::Null;
        }
        self_service::gate(
            &tx,
            row["session_context"]["operator"] == true,
            &gated_action,
            &claims.plugin_id,
            string(&row, "customer_id")?,
            &claims.project_id,
            row["session_context"]["resourceID"].as_str(),
        )
        .await?;
        tx.commit().await.map_err(db_error)?;
        Ok((
            PersistedInvocation {
                id: claims.id.clone(),
                actor,
                session,
                row,
            },
            metadata,
            binding,
        ))
    }
    pub(crate) async fn authorized(
        &self,
        handle: &InvocationHandle,
        capability: Option<&str>,
    ) -> Result<Authorized> {
        let (inv, metadata, binding) = self.persisted(&handle.claims).await?;
        if !binding.permits(&handle.peer) {
            return denied();
        }
        if let Some(capability) = capability {
            let service = capability
                .split('.')
                .nth(1)
                .ok_or_else(PluginError::denied)?;
            if !["required", "optional"].iter().any(|key| {
                metadata.manifest.as_json()["services"][key]
                    .as_array()
                    .is_some_and(|list| list.iter().any(|entry| entry["name"] == service))
            }) {
                return denied();
            }
        }
        let stored = inv.row["session_context"]
            .get("authority")
            .filter(|v| !v.is_null())
            .map(|value| {
                serde_json::from_value::<AuthorityReference>(value.clone())
                    .map_err(|_| PluginError::denied())
            })
            .transpose()?;
        let (auth_tx, principal) = if let Some(stored) = &stored {
            let (tx, principal) = self
                .session_authority
                .as_ref()
                .ok_or_else(PluginError::denied)?
                .begin_principal(
                    &inv.actor,
                    &inv.session,
                    stored,
                    inv.row["intent"] == "write",
                )
                .await?;
            (Some(tx), Some(principal))
        } else {
            (None, None)
        };
        let tx = self.db.begin().await.map_err(db_error)?;
        self.scopes.invocation(&tx, &inv).await?;
        timeouts(&tx).await?;
        fence(&tx, false).await?;
        Self::lock_invocation(&tx).await?;
        let scope = Self::scope(&tx, capability).await?;
        let project_customer =
            Self::project(&tx, &handle.claims.project_id, inv.row["intent"] == "write").await?;
        if handle.claims.plugin_id == "neon" {
            if !matches!(stored.as_ref(), Some(AuthorityReference::Browser { .. })) {
                return denied();
            }
            Self::neon_scope(
                &tx,
                &project_customer,
                inv.row["intent"] == "write",
                &handle.claims.action,
            )
            .await?;
        }
        if scope["pluginID"] != handle.claims.plugin_id
            || scope["projectID"] != handle.claims.project_id
            || scope["customerID"] != project_customer
            || scope["operationID"] != inv.row["operation_id"]
        {
            return denied();
        }
        Ok(Authorized {
            tx,
            auth_tx,
            authority_ref: stored,
            intent_write: inv.row["intent"] == "write",
            principal,
            invocation: inv.row.clone(),
            binding_hash: string(
                &inv.row["session_context"]["nativeAdmission"],
                "bindingSHA256",
            )?
            .into(),
            scope: InvocationScope {
                plugin_id: handle.claims.plugin_id.clone(),
                customer_id: project_customer,
                project_id: handle.claims.project_id.clone(),
                operation_id: string(&scope, "operationID")?.into(),
                subject: inv.actor,
                session_id: inv.session,
                generation: handle.claims.generation,
                grant_revision: integer(&inv.row, "grant_revision")?,
            },
            binding,
            manifest: metadata.manifest,
        })
    }
    async fn scope(tx: &DatabaseTransaction, capability: Option<&str>) -> Result<Value> {
        tx.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT webdock_rls.plugin_context($1) AS scope",
            [capability.into()],
        ))
        .await
        .map_err(db_error)?
        .ok_or_else(PluginError::denied)?
        .try_get::<Option<Value>>("", "scope")
        .map_err(db_error)?
        .ok_or_else(PluginError::denied)
    }
    async fn lock_invocation(tx: &DatabaseTransaction) -> Result<()> {
        let row = tx
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT webdock_rls.lock_plugin_invocation() AS allowed",
            ))
            .await
            .map_err(db_error)?
            .ok_or_else(PluginError::denied)?;
        if !row.try_get::<bool>("", "allowed").map_err(db_error)? {
            return denied();
        }
        Ok(())
    }
    pub(crate) async fn finish(
        &self,
        authorized: Authorized,
        handle: &InvocationHandle,
        capability: Option<&str>,
    ) -> Result<()> {
        // The business transaction already holds the metadata fence. Do not reacquire the
        // controller pool: a revoker may be waiting there for this invocation's shared lock.
        Self::lock_invocation(&authorized.tx).await?;
        Self::project(
            &authorized.tx,
            &handle.claims.project_id,
            authorized.intent_write,
        )
        .await?;
        Self::scope(&authorized.tx, capability).await?;
        if handle.claims.plugin_id == "neon" {
            Self::neon_scope(
                &authorized.tx,
                &authorized.scope.customer_id,
                authorized.intent_write,
                &handle.claims.action,
            )
            .await?;
        }
        let binding = self
            .bindings
            .runtime(&handle.claims.plugin_id)
            .await?
            .ok_or_else(PluginError::denied)?;
        if binding.generation != handle.claims.generation
            || !binding.permits(&handle.peer)
            || digest(serde_json::to_vec(&binding).map_err(|_| unavailable())?)
                != authorized.binding_hash
        {
            return denied();
        }
        if let (Some(auth), Some(authority)) = (&authorized.auth_tx, &authorized.authority_ref) {
            self.session_authority
                .as_ref()
                .ok_or_else(PluginError::denied)?
                .verify(
                    auth,
                    &authorized.scope.subject,
                    &authorized.scope.session_id,
                    authority,
                    authorized.intent_write,
                )
                .await?;
        }
        authorized.tx.commit().await.map_err(db_error)?;
        if let Some(auth) = authorized.auth_tx {
            auth.commit().await.map_err(db_error)?;
        }
        Ok(())
    }
    fn current_configuration(
        metadata: &Metadata,
        binding: &RuntimeBinding,
        project: &str,
    ) -> Result<Value> {
        let mut configuration = metadata.configuration["configuration"]
            .as_object()
            .cloned()
            .ok_or_else(unavailable)?;
        if let Some(overrides) = binding.public_configuration.get(project) {
            configuration.extend(overrides.as_object().ok_or_else(unavailable)?.clone());
        }
        let configuration = crate::data::legacy_json_value(&Value::Object(configuration))?;
        validate_value(
            Some(&metadata.manifest.as_json()["configuration"]),
            &configuration,
        )?;
        Ok(configuration)
    }
    fn admission_snapshot(
        metadata: &Metadata,
        binding: &RuntimeBinding,
        configuration: &Value,
    ) -> Result<Value> {
        if serde_json::to_vec(configuration)
            .map_err(|_| unavailable())?
            .len()
            > 65536
            || serde_json::to_vec(&metadata.configuration["configuration"])
                .map_err(|_| unavailable())?
                .len()
                > 65536
        {
            return Err(invalid("Plugin configuration exceeds 64 KiB"));
        }
        Ok(
            json!({"v":1,"configurationRevision":metadata.configuration["revision"],"configuration":metadata.configuration["configuration"],"runtimeCredentialSHA256":metadata.runtime["credential_hash"],"runtimeNamespace":metadata.runtime["namespace"],"runtimeService":metadata.runtime["service_name"],"configurationSHA256":digest(serde_json::to_vec(configuration).map_err(|_|unavailable())?),"manifestSHA256":metadata.release["manifest_sha256"],"bindingSHA256":digest(serde_json::to_vec(binding).map_err(|_|unavailable())?)}),
        )
    }
    /// Bind the ordinary v1 token's separately persisted payload to this exact RPC request.
    /// The signed envelope remains unchanged; old unpinned invocations expire closed.
    pub async fn verify_request(
        &self,
        handle: &InvocationHandle,
        input: &Value,
        configuration: &Value,
    ) -> Result<()> {
        let (inv, _, _) = self.persisted(&handle.claims).await?;
        let request_bytes = if handle.claims.job.is_some() {
            jobs::canonical_json(input)?
        } else {
            crate::data::javascript_json(input, false)?.into_bytes()
        };
        if request_bytes.len()
            > invocation_input_limit(
                &handle.claims.plugin_id,
                &handle.claims.action,
                handle.claims.job.is_some(),
            )
            || serde_json::to_vec(configuration)
                .map_err(|_| unavailable())?
                .len()
                > 65536
            || inv.row["request_hash"] != digest(&request_bytes)
            || inv.row["session_context"]["nativeAdmission"]["configurationSHA256"]
                != digest(serde_json::to_vec(configuration).map_err(|_| unavailable())?)
        {
            return denied();
        }
        self.revalidate(handle).await
    }
    pub async fn revalidate(&self, handle: &InvocationHandle) -> Result<()> {
        let context = self.authorized(handle, None).await?;
        self.finish(context, handle, None).await
    }
    /// Issuer cleanup after an invocation attempt, including when its live actor was revoked.
    /// Possession of a valid Core-signed token permits only revocation of that exact invocation.
    pub async fn revoke_issued(&self, token: &str) -> Result<()> {
        if token.len() > 16384 {
            return denied();
        }
        let body = token.split_once('.').ok_or_else(PluginError::denied)?.0;
        let hint: InvocationClaims = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(body)
                .map_err(|_| PluginError::denied())?,
        )
        .map_err(|_| PluginError::denied())?;
        // Cleanup may run exactly at expiry; signature/scope are verified at the issued time.
        let claims = verify_invocation(
            token,
            &self.key.verifying_key(),
            &hint.plugin_id,
            hint.generation,
            hint.iat,
        )?;
        let tx = self.controller().await?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_invocation SET revoked_at=coalesce(revoked_at,now()) WHERE id=$1::uuid AND plugin_id=$2 AND project_id=$3 AND runtime_generation=$4 AND name=$5",[claims.id.into(),claims.plugin_id.into(),claims.project_id.into(),(claims.generation as i64).into(),claims.action.into()])).await.map_err(db_error)?;
        tx.commit().await.map_err(db_error)
    }
    pub async fn revoke(&self, handle: &InvocationHandle) -> Result<()> {
        self.revalidate(handle).await?;
        let tx = self.controller().await?;
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_auth.plugin_invocation SET revoked_at=now() WHERE id=$1::uuid",
            [handle.claims.id.clone().into()],
        ))
        .await
        .map_err(db_error)?;
        tx.commit().await.map_err(db_error)
    }
    pub async fn apps_request(&self, handle: &InvocationHandle, input: Value) -> Result<Value> {
        let operation = input
            .get("operation")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("Invalid managed app request"))?;
        let capability = match operation {
            "list" | "get" | "provisioningStatus" | "provisioningList" => "plugin.apps.read",
            "command" => "plugin.apps.manage",
            "provision" => "plugin.apps.provision",
            _ => return Err(invalid("Invalid managed app request")),
        };
        if input.as_object().is_none_or(|o| {
            o.keys()
                .any(|k| !["operation", "input"].contains(&k.as_str()))
        }) || serde_json::to_vec(&input)
            .map_err(|_| invalid("Invalid managed app request"))?
            .len()
            > 32768
        {
            return Err(invalid("Invalid managed app request"));
        }
        let context = self.authorized(handle, Some(capability)).await?;
        let scope = context.scope.clone();
        self.finish(context, handle, Some(capability)).await?;
        let adapter = self
            .apps
            .as_ref()
            .ok_or_else(|| unsupported("Managed apps are unavailable"))?;
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            adapter.request(&scope, input),
        )
        .await
        .map_err(|_| unavailable())??;
        let context = self.authorized(handle, Some(capability)).await?;
        self.finish(context, handle, Some(capability)).await?;
        if serde_json::to_vec(&result)
            .map_err(|_| unavailable())?
            .len()
            > 65536
        {
            return Err(unavailable());
        }
        Ok(result)
    }
}
pub struct RuntimeAuthority {
    host: Arc<HostService>,
    plugin_id: String,
    generation: u64,
}
#[async_trait]
impl InvocationAuthority for RuntimeAuthority {
    async fn authorize_request(
        &self,
        peer: &PeerIdentity,
        claims: &InvocationClaims,
        input: &[u8],
        configuration: &[u8],
    ) -> Result<()> {
        if claims.plugin_id != self.plugin_id || claims.generation != self.generation {
            return denied();
        }
        self.host
            .verify_request(
                &InvocationHandle {
                    peer: peer.clone(),
                    claims: claims.clone(),
                },
                &serde_json::from_slice(input)
                    .map_err(|_| invalid("Invalid invocation payload"))?,
                &serde_json::from_slice(configuration)
                    .map_err(|_| invalid("Invalid invocation configuration"))?,
            )
            .await
    }

    async fn authorize_peer(&self, peer: &PeerIdentity) -> Result<()> {
        let tx = self.host.controller().await?;
        let metadata = self.host.metadata(&tx, &self.plugin_id, None).await?;
        let binding = self.host.binding(&self.plugin_id, &metadata).await?;
        if binding.generation != self.generation
            || !binding.permits(peer)
            || metadata.installation["state"] == "disabled"
        {
            return denied();
        }
        tx.commit().await.map_err(db_error)
    }
    async fn authorize_invocation(
        &self,
        peer: &PeerIdentity,
        claims: &InvocationClaims,
    ) -> Result<()> {
        if claims.plugin_id != self.plugin_id || claims.generation != self.generation {
            return denied();
        }
        self.host
            .revalidate(&InvocationHandle {
                peer: peer.clone(),
                claims: claims.clone(),
            })
            .await
    }
}
pub(crate) async fn timeouts(tx: &DatabaseTransaction) -> Result<()> {
    tx.query_one_raw(Statement::from_string(
        DbBackend::Postgres,
        "SELECT set_config('lock_timeout','3s',true),set_config('statement_timeout','5s',true)",
    ))
    .await
    .map_err(db_error)?;
    Ok(())
}
pub(crate) async fn fence(tx: &DatabaseTransaction, exclusive: bool) -> Result<()> {
    tx.query_one_raw(Statement::from_string(
        DbBackend::Postgres,
        if exclusive {
            "SELECT pg_advisory_xact_lock(hashtext('webdock-plugin-installation'))"
        } else {
            "SELECT pg_advisory_xact_lock_shared(hashtext('webdock-plugin-installation'))"
        },
    ))
    .await
    .map_err(db_error)?;
    Ok(())
}
#[path = "admin.rs"]
mod admin;
#[path = "builtins.rs"]
pub mod builtins;
#[path = "controller.rs"]
mod controller;
#[path = "installation.rs"]
pub mod installation;
#[path = "jobs.rs"]
pub mod jobs;
#[path = "limits.rs"]
pub mod limits;
#[path = "notifications.rs"]
mod notifications;
#[path = "placement.rs"]
mod placement;
#[path = "self_service.rs"]
mod self_service;
#[path = "workloads.rs"]
pub mod workloads;
pub use controller::{
    OperationSnapshot, OperationStatus, RuntimeDescription, StateLease, StateProbe,
};

/// A momentary metadata snapshot, NOT durable authorization. Repository Core must separately
/// validate its persisted domain lease and request this snapshot again on every live callback.
pub struct RepositoryPolicySnapshot {
    pub plugin_id: String,
    pub version: String,
    pub customer_id: String,
    pub project_id: String,
    pub generation: u64,
    pub grant_revision: u64,
    pub configuration_revision: u64,
    pub configuration: Value,
    pub manifest_sha256: String,
    pub image: String,
    pub registry_url: String,
    pub signer_key_id: String,
    pub binding: RuntimeBinding,
}
impl HostService {
    /// Backend-only fixed repository service lane. Does not create or alter ordinary Invoke grants.
    pub async fn repository_policy(
        &self,
        plugin: &str,
        project: &str,
        action: &str,
        input: &Value,
        continuation: bool,
    ) -> Result<RepositoryPolicySnapshot> {
        if ![
            "repository.resolve",
            "repository.archive",
            "repository.check",
        ]
        .contains(&action)
            || !slug(plugin)
            || project.is_empty()
            || !project.bytes().all(|b| b.is_ascii_digit())
            || serde_json::to_vec(input)
                .map_err(|_| invalid("Invalid repository input"))?
                .len()
                > 32768
        {
            return Err(invalid("Invalid repository policy request"));
        }
        let tx = self.controller().await?;
        fence(&tx, false).await?;
        let metadata = self.metadata(&tx, plugin, Some(project)).await?;
        let binding = self.binding(plugin, &metadata).await?;
        if metadata.installation["state"] == "disabled"
            || (!continuation && metadata.installation["state"] != "enabled")
            || (!continuation && metadata.runtime["status"] != "ready")
        {
            return denied();
        }
        let selected = self.action(&metadata, action)?;
        validate_value(selected.get("input"), input)?;
        let mut config = metadata.configuration["configuration"]
            .as_object()
            .cloned()
            .ok_or_else(unavailable)?;
        if let Some(overrides) = binding.public_configuration.get(project) {
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
            return Err(invalid("Repository configuration exceeds limit"));
        }
        let snapshot = RepositoryPolicySnapshot {
            plugin_id: plugin.into(),
            version: string(&metadata.installation, "version")?.into(),
            customer_id: string(&metadata.grant, "customer_id")?.into(),
            project_id: project.into(),
            generation: integer(&metadata.runtime, "generation")? as u64,
            grant_revision: integer(&metadata.grant, "revision")? as u64,
            configuration_revision: integer(&metadata.configuration, "revision")? as u64,
            configuration,
            manifest_sha256: string(&metadata.release, "manifest_sha256")?.into(),
            image: string(&metadata.release, "image")?.into(),
            registry_url: string(&metadata.release, "registry_url")?.into(),
            signer_key_id: string(&metadata.release, "signer_key_id")?.into(),
            binding,
        };
        tx.commit().await.map_err(db_error)?;
        Ok(snapshot)
    }
}

impl RepositoryPolicySnapshot {
    pub fn permits_peer(&self, peer: &PeerIdentity) -> bool {
        self.binding.permits(peer)
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginView {
    pub state: String,
    pub runtime: String,
    pub manifest: Value,
    pub generation: u64,
    pub manifest_sha256: String,
    pub launch_links: Vec<Value>,
    /// Self-service features this actor may use in the project; operators get all of them.
    pub features: Vec<String>,
    /// New-create eligibility is separate from feature grants used for retained maintenance.
    pub blocked_create_actions: Vec<String>,
    /// The project's resources of this plugin from the Core index.
    pub resources: Vec<Value>,
}
impl HostService {
    /// Public metadata scoped by a current project permission and current installation/grant.
    /// Discovery never substitutes for admission at execution time.
    pub async fn project_plugins_with_authority(
        &self,
        actor: &Principal,
        project: &str,
        authority: &spitzli_browser_session::StoredAuthority,
    ) -> Result<Vec<PluginView>> {
        self.project_plugins_reference(
            actor,
            project,
            &AuthorityReference::Browser {
                authority: authority.clone(),
            },
        )
        .await
    }
    pub async fn project_plugins_with_mcp(
        &self,
        actor: &Principal,
        project: &str,
        authority: &spitzli_oauth::McpAuthority,
    ) -> Result<Vec<PluginView>> {
        self.project_plugins_reference(
            actor,
            project,
            &AuthorityReference::Mcp {
                authority: authority.clone(),
            },
        )
        .await
    }
    async fn project_plugins_reference(
        &self,
        actor: &Principal,
        project: &str,
        authority: &AuthorityReference,
    ) -> Result<Vec<PluginView>> {
        let verifier = self
            .session_authority
            .as_ref()
            .ok_or_else(PluginError::denied)?;
        let auth = verifier
            .begin(&actor.user_id, &actor.session_id, authority, false)
            .await?;
        let result = self.project_plugins(actor, project).await?;
        verifier
            .verify(&auth, &actor.user_id, &actor.session_id, authority, false)
            .await?;
        auth.commit().await.map_err(db_error)?;
        Ok(result)
    }
    pub async fn project_plugins(
        &self,
        actor: &Principal,
        project: &str,
    ) -> Result<Vec<PluginView>> {
        if project.is_empty()
            || project.starts_with('0')
            || !project.bytes().all(|b| b.is_ascii_digit())
            || !project.parse::<i64>().is_ok_and(|n| n > 0)
        {
            return Err(invalid("Invalid project"));
        }
        let session = self
            .signer
            .begin(&self.db, actor)
            .await
            .map_err(|_| PluginError::denied())?;
        timeouts(&session).await?;
        let customer = Self::project(&session, project, false).await?;
        let writer = session
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.customer_allowed($1,true) AS writer",
                [customer.clone().into()],
            ))
            .await
            .map_err(db_error)?
            .ok_or_else(PluginError::denied)?
            .try_get::<bool>("", "writer")
            .map_err(db_error)?;
        let operator: bool = session
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT webdock_rls.operator() AS operator",
            ))
            .await
            .map_err(db_error)?
            .ok_or_else(PluginError::denied)?
            .try_get("", "operator")
            .map_err(db_error)?;
        let tx = self.controller().await?;
        fence(&tx, false).await?;
        let ids=tx.query_all_raw(Statement::from_string(DbBackend::Postgres,"SELECT id FROM webdock_auth.plugin_installation WHERE kind='external' AND state='enabled' AND runtime='ready' ORDER BY id")).await.map_err(db_error)?;
        let mut views = Vec::new();
        for id in ids {
            let id: String = id.try_get("", "id").map_err(db_error)?;
            self_service::ensure_grant(&tx, &id, project).await?;
            // An installation this Core cannot run (no runtime binding, unreadable release) is not
            // listed; it must not hide the others. Admission rechecks everything per call.
            let Ok(metadata) = self.metadata(&tx, &id, Some(project)).await else {
                continue;
            };
            let Ok(binding) = self.binding(&id, &metadata).await else {
                continue;
            };
            if metadata.grant["customer_id"] != customer || !metadata.grant["revoked_at"].is_null()
            {
                return denied();
            }
            let features = if operator {
                self_service::supported_features(&tx, &metadata.manifest, &id).await?
            } else if writer {
                self_service::usable_features(&tx, &metadata.manifest, &id, &customer).await?
            } else {
                vec![]
            };
            let resources = self_service::project_resources(&tx, &id, project).await?;
            // Customers only see plugins they can do something with in this project.
            if !operator && features.is_empty() && resources.is_empty() {
                continue;
            }
            views.push(PluginView {
                state: string(&metadata.installation, "state")?.into(),
                runtime: string(&metadata.runtime, "status")?.into(),
                manifest: metadata.manifest.as_json().clone(),
                generation: binding.generation,
                manifest_sha256: string(&metadata.release, "manifest_sha256")?.into(),
                launch_links: vec![],
                features,
                blocked_create_actions: self.blocked_database_creates(&metadata.manifest),
                resources,
            });
        }
        Self::project(&session, project, false).await?;
        tx.commit().await.map_err(db_error)?;
        session.commit().await.map_err(db_error)?;
        Ok(views)
    }
}

#[path = "mail_policy.rs"]
mod mail_policy;
#[path = "native_mail.rs"]
mod native_mail;
pub use mail_policy::MailPolicySnapshot;
pub use native_mail::{MailActionGuard, MailRuntimeMode, MailRuntimeProof};

impl HostService {
    /// MCP discovery is operator-scoped metadata; each call still requires a current project grant.
    pub async fn mcp_plugins(
        &self,
        actor: &Principal,
        authority: &spitzli_oauth::McpAuthority,
    ) -> Result<Vec<PluginView>> {
        let reference = AuthorityReference::Mcp {
            authority: authority.clone(),
        };
        let verifier = self
            .session_authority
            .as_ref()
            .ok_or_else(PluginError::denied)?;
        let auth = verifier
            .begin(&actor.user_id, &actor.session_id, &reference, false)
            .await?;
        let tx = self.controller().await?;
        fence(&tx, false).await?;
        let ids=tx.query_all_raw(Statement::from_string(DbBackend::Postgres,"SELECT id FROM webdock_auth.plugin_installation WHERE kind='external' AND state='enabled' AND runtime='ready' ORDER BY id")).await.map_err(db_error)?;
        let mut views = Vec::new();
        for row in ids {
            let plugin: String = row.try_get("", "id").map_err(db_error)?;
            // Same rule as project discovery: skip installations this Core cannot run.
            let Ok(metadata) = self.metadata(&tx, &plugin, None).await else {
                continue;
            };
            let Ok(binding) = self.binding(&plugin, &metadata).await else {
                continue;
            };
            views.push(PluginView {
                state: "enabled".into(),
                runtime: "ready".into(),
                manifest: metadata.manifest.as_json().clone(),
                generation: binding.generation,
                manifest_sha256: string(&metadata.release, "manifest_sha256")?.into(),
                launch_links: vec![],
                features: vec![],
                blocked_create_actions: self.blocked_database_creates(&metadata.manifest),
                resources: vec![],
            });
        }
        verifier
            .verify(&auth, &actor.user_id, &actor.session_id, &reference, false)
            .await?;
        tx.commit().await.map_err(db_error)?;
        auth.commit().await.map_err(db_error)?;
        Ok(views)
    }
}

#[cfg(test)]
mod mail_input_budget_tests {
    #[test]
    fn large_mail_budget_is_only_for_original_send_actions() {
        use super::invocation_input_limit;
        assert_eq!(invocation_input_limit("mail", "mail.send", false), 131072);
        assert_eq!(invocation_input_limit("mail", "mail.send", true), 32768);
        assert_eq!(invocation_input_limit("mail", "mail.status", false), 32768);
        assert_eq!(invocation_input_limit("other", "mail.send", false), 32768);
    }
}

#[path = "storage_admission.rs"]
pub(crate) mod storage_admission;

#[path = "exports.rs"]
pub mod exports;

impl Authorized {
    pub(crate) fn http_adapter_context<'a>(
        &mut self,
        host: &'a HostService,
        handle: &'a InvocationHandle,
    ) -> Result<crate::http::HttpAdapterContext<'a>> {
        let Some(AuthorityReference::Browser { authority }) = &self.authority_ref else {
            return denied();
        };
        if self.scope.plugin_id != "neon" || self.invocation["kind"] != "action" {
            return denied();
        }
        Ok(crate::http::HttpAdapterContext {
            host,
            handle,
            scope: self.scope.clone(),
            invocation_id: string(&self.invocation, "id")?.into(),
            action: string(&self.invocation, "name")?.into(),
            write: self.intent_write,
            request_hash: string(&self.invocation, "request_hash")?.into(),
            resource_id: self.invocation["session_context"]["resourceID"]
                .as_str()
                .map(str::to_owned),
            principal: self.principal.take().ok_or_else(PluginError::denied)?,
            authority: authority.clone(),
        })
    }
}

impl HostService {
    async fn neon_scope(
        tx: &DatabaseTransaction,
        customer: &str,
        write: bool,
        action: &str,
    ) -> Result<()> {
        let allowed = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.neon_connection_allowed($1,$2) AS allowed",
                [customer.into(), neon_management(write, action).into()],
            ))
            .await
            .map_err(db_error)?
            .ok_or_else(PluginError::denied)?
            .try_get::<bool>("", "allowed")
            .map_err(db_error)?;
        if !allowed {
            return denied();
        }
        Ok(())
    }
}
