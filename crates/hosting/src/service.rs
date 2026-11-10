use crate::{
    Result,
    contracts::{self, DIMENSIONS, amount},
    environment::Environment,
    error,
};
use hmac::{Hmac, Mac};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, Statement,
    TransactionTrait, Value as SqlValue,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use spitzli_identity::{Principal, RlsSigner};
use spitzli_persistence::IdAllocator;
use std::sync::Arc;
#[derive(Clone, Copy)]
pub struct HostingCeiling {
    pub read: bool,
    pub write: bool,
}
impl Default for HostingCeiling {
    fn default() -> Self {
        Self {
            read: true,
            write: true,
        }
    }
}
pub struct HostingService {
    db: DatabaseConnection,
    auth: DatabaseConnection,
    worker: DatabaseConnection,
    signer: RlsSigner,
    rls_secret: String,
    environment: Environment,
    ids: Arc<IdAllocator>,
    browser_authority: Option<Arc<browser_authority::BrowserVerifier>>,
    recovery_signer: Option<crate::RecoverySigner>,
    public_site_origins: Vec<String>,
    autoscaling_enabled: bool,
}
// Internal actor is either a live browser principal or a persisted, validated Git release.
struct HostingActor {
    user_id: String,
    session_id: String,
    git_release: Option<String>,
    customer_scope: Option<String>,
}
impl From<&Principal> for HostingActor {
    fn from(p: &Principal) -> Self {
        Self {
            user_id: p.user_id.clone(),
            session_id: p.session_id.clone(),
            git_release: None,
            customer_scope: p.customer_scope().map(str::to_owned),
        }
    }
}
#[derive(Clone)]
struct Access {
    customer: String,
    project: Value,
    operator: bool,
    live: bool,
    can_write: bool,
}
#[derive(Clone)]
pub struct AgentIdentity {
    cluster: String,
    generation: i32,
    credential_hash: String,
}
impl AgentIdentity {
    pub fn cluster_id(&self) -> &str {
        &self.cluster
    }
    pub fn generation(&self) -> i32 {
        self.generation
    }
}
fn recovery_sqlstate(code: Option<std::borrow::Cow<'_, str>>) -> Option<String> {
    code.filter(|value| {
        value.len() == 5
            && value
                .bytes()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    })
    .map(|value| value.into_owned())
}
fn recovery_db_observation(error: &sea_orm::DbErr) -> (&'static str, Option<String>) {
    use sea_orm::{DbErr, RuntimeErr};
    let class = match error {
        DbErr::Query(_) => "query",
        DbErr::Exec(_) => "execute",
        DbErr::Conn(_) => "connection",
        DbErr::Type(_) => "decode",
        _ => "other",
    };
    let code = match error {
        DbErr::Query(RuntimeErr::SqlxError(error))
        | DbErr::Exec(RuntimeErr::SqlxError(error))
        | DbErr::Conn(RuntimeErr::SqlxError(error)) => {
            error.as_database_error().and_then(|value| value.code())
        }
        _ => None,
    };
    (class, recovery_sqlstate(code))
}
fn db_error(cause: sea_orm::DbErr) -> crate::HostingError {
    if std::env::var("SPITZLI_RECOVERY_COMPONENT_TEST").as_deref() == Ok("1") {
        let (class, state) = recovery_db_observation(&cause);
        eprintln!(
            "recovery_component database_failure class={class} sqlstate={}",
            state.as_deref().unwrap_or("unavailable")
        );
    }
    error(
        503,
        "Hosting is unavailable. Check the current state before retrying.",
    )
}
async fn one(
    db: &impl ConnectionTrait,
    sql: &str,
    values: Vec<SqlValue>,
) -> Result<Option<sea_orm::QueryResult>> {
    db.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .map_err(db_error)
}
async fn rows(db: &impl ConnectionTrait, sql: &str, values: Vec<SqlValue>) -> Result<Vec<Value>> {
    db.query_all_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .map_err(db_error)?
    .into_iter()
    .map(|row| row.try_get("", "value").map_err(db_error))
    .collect()
}
async fn execute(db: &impl ConnectionTrait, sql: &str, values: Vec<SqlValue>) -> Result<u64> {
    Ok(db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .await
        .map_err(db_error)?
        .rows_affected())
}
fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .ok_or_else(|| error(503, "Stored hosting state is invalid."))
}
fn numeric(value: &Value, key: &str) -> Result<i64> {
    value[key]
        .as_i64()
        .or_else(|| {
            value[key]
                .as_f64()
                .filter(|v| v.fract() == 0.0 && *v >= 0.0 && *v <= contracts::MAX_SAFE as f64)
                .map(|v| v as i64)
        })
        .ok_or_else(|| error(503, "Stored hosting state is invalid."))
}
fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
fn epoch(value: &Value) -> Option<i64> {
    value
        .as_str()
        .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
        .map(|v| v.timestamp())
}
fn timestamp(value: &Value) -> Value {
    value
        .as_str()
        .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
        .map(|v| {
            json!(
                v.to_utc()
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            )
        })
        .unwrap_or(Value::Null)
}
impl HostingService {
    pub async fn new(
        db: DatabaseConnection,
        auth: DatabaseConnection,
        worker: DatabaseConnection,
        rls_secret: String,
        environment_secret: String,
        ids: Arc<IdAllocator>,
    ) -> Result<Self> {
        for (connection, role) in [
            (&db, "webdock_auth_runtime"),
            (&auth, "webdock_auth_bootstrap"),
            (&worker, "webdock_worker_runtime"),
        ] {
            spitzli_persistence::connection::verify_runtime_role(connection, role)
                .await
                .map_err(|_| error(503, "Restricted hosting database roles are required."))?;
        }
        let signer = RlsSigner::new(rls_secret.clone(), "webdock_auth_runtime")
            .map_err(|_| error(503, "Invalid RLS configuration."))?;
        Ok(Self {
            db,
            auth,
            worker,
            signer,
            rls_secret,
            environment: Environment::new(environment_secret)?,
            ids,
            browser_authority: None,
            recovery_signer: None,
            public_site_origins: Vec::new(),
            autoscaling_enabled: std::env::var("SPITZLI_HOSTING_AUTOSCALING_ENABLED").as_deref()
                == Ok("true"),
        })
    }
    async fn automatic_scaling_available(
        &self,
        tx: &DatabaseTransaction,
        cluster: Option<&str>,
    ) -> Result<bool> {
        if !self.autoscaling_enabled {
            return Ok(false);
        }
        let Some(cluster) = cluster else {
            return Ok(false);
        };
        let snapshot = Self::cluster_view(tx, cluster).await?;
        Ok(snapshot["runtimeKind"] == "kubernetes"
            && snapshot["observation"]["capabilities"]["autoscalingVersion"] == 1)
    }
    async fn id(&self) -> Result<String> {
        Ok(self
            .ids
            .allocate()
            .await
            .map_err(|_| error(503, "ID allocation unavailable."))?
            .to_string())
    }
    async fn begin(&self, actor: &Principal) -> Result<DatabaseTransaction> {
        let tx = self
            .signer
            .begin(&self.db, actor)
            .await
            .map_err(|_| error(401, "Sign in and complete account setup."))?;
        one(&tx,"SELECT set_config('lock_timeout','5s',true),set_config('statement_timeout','10s',true)",vec![]).await?;
        Ok(tx)
    }
    async fn module(tx: &DatabaseTransaction, write: bool, continuation: bool) -> Result<()> {
        one(
            tx,
            "SELECT pg_advisory_xact_lock_shared(hashtext('webdock-plugin-installation'))",
            vec![],
        )
        .await?;
        let row = one(
            tx,
            "SELECT state FROM webdock_auth.plugin_installation WHERE id='hosting'",
            vec![],
        )
        .await?
        .ok_or_else(|| error(503, "Hosting installation is unavailable."))?;
        let state: String = row.try_get("", "state").map_err(db_error)?;
        if state == "disabled" || (state == "draining" && write && !continuation) {
            return Err(error(503, "Hosting is disabled or draining."));
        }
        Ok(())
    }
    async fn authorize(
        tx: &DatabaseTransaction,
        actor: &HostingActor,
        project: Option<&str>,
        customer: Option<&str>,
        write: bool,
        operator: bool,
        live: bool,
    ) -> Result<Access> {
        if let Some(release) = &actor.git_release {
            return Self::authorize_git(
                tx, actor, release, project, customer, write, operator, live,
            )
            .await;
        }
        let identity: Value = one(tx, "SELECT webdock_rls.actor() AS actor", vec![])
            .await?
            .ok_or_else(|| error(401, "Authentication required."))?
            .try_get::<Option<Value>>("", "actor")
            .map_err(db_error)?
            .ok_or_else(|| error(401, "Sign in and complete account setup."))?;
        if identity["subject"] != actor.user_id || identity["sessionID"] != actor.session_id {
            return Err(error(401, "Identity mismatch."));
        }
        let is_operator = identity["operator"] == true;
        let acting: bool = one(
            tx,
            "SELECT webdock_rls.acting_customer() IS NOT NULL AS acting",
            vec![],
        )
        .await?
        .ok_or_else(|| error(403, "Customer authority is unavailable."))?
        .try_get("", "acting")
        .map_err(db_error)?;
        let preview = !identity["preview"].is_null() && !acting;
        if preview && (write || operator || live) {
            return Err(error(403, "Customer preview is read-only."));
        }
        if operator && !is_operator {
            return Err(error(403, "Platform operator access is required."));
        }
        let mut target = Value::Null;
        let mut customer = customer.map(str::to_owned);
        if let Some(project) = project {
            contracts::record_id(project)?;
            one(
                tx,
                "SELECT webdock_admin.lock_hosting_project($1)",
                vec![project.into()],
            )
            .await?;
            target=rows(tx,"SELECT to_jsonb(p)||coalesce(to_jsonb(h),'{}'::jsonb) AS value FROM webdock_admin.projects p LEFT JOIN webdock_auth.hosting_project h ON h.project_id=p.id WHERE p.id=$1 AND p.status='active' AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=p.id AND d.status<>'deleted')",vec![project.into()]).await?.pop().ok_or_else(||error(404,"Hosting target is unavailable."))?;
            let owner = field(&target, "customer_id")?;
            if customer.as_ref().is_some_and(|c| c != owner) {
                return Err(error(404, "Hosting target is unavailable."));
            }
            customer = Some(owner.into());
        }
        let mut can_write = is_operator;
        if let Some(customer) = &customer {
            contracts::record_id(customer)?;
            let row=one(tx,"SELECT webdock_rls.customer_allowed(c.id,false) AS allowed,webdock_rls.customer_permission(c.id,'org:projects:manage') AS can_write FROM webdock_admin.customers c JOIN webdock_auth.tenant_customer t ON t.customer_id=c.id WHERE c.id=$1 AND c.status='active' FOR SHARE OF c",vec![customer.clone().into()]).await?.ok_or_else(||error(404,"Hosting target is unavailable."))?;
            if !row.try_get::<bool>("", "allowed").map_err(db_error)? {
                return Err(error(404, "Hosting target is unavailable."));
            }
            can_write = row.try_get("", "can_write").map_err(db_error)?;
            if write && (!can_write || (!is_operator && target["mode"] != "selfservice")) {
                return Err(error(
                    403,
                    "This hosting action is managed by your operator.",
                ));
            }
        } else if !is_operator {
            return Err(error(403, "Choose an authorized hosting customer."));
        }
        Ok(Access {
            customer: customer.unwrap_or_default(),
            project: target,
            operator: is_operator,
            live: !preview,
            can_write: !preview && can_write,
        })
    }
    async fn lock(tx: &DatabaseTransaction, key: String) -> Result<()> {
        one(
            tx,
            "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
            vec![key.into()],
        )
        .await?;
        Ok(())
    }
    async fn audit(
        &self,
        tx: &DatabaseTransaction,
        subject: &str,
        action: &str,
        target: &str,
        outcome: &str,
    ) -> Result<()> {
        execute(tx,"INSERT INTO webdock_auth.hosting_audit(id,subject,action,target_id,outcome) VALUES($1,$2,$3,$4,$5)",vec![self.id().await?.into(),subject.into(),action.into(),target.into(),outcome.into()]).await?;
        Ok(())
    }
    pub async fn execute(&self, actor: &Principal, input: Value) -> Result<Value> {
        self.execute_with_ceiling(actor, input, HostingCeiling::default())
            .await
    }
    pub async fn execute_with_ceiling(
        &self,
        actor: &Principal,
        input: Value,
        ceiling: HostingCeiling,
    ) -> Result<Value> {
        self.execute_guarded(actor, input, ceiling, None).await
    }
    /// Preserve the token ceiling and live access record through the domain commit.
    pub async fn execute_with_mcp(
        &self,
        oauth: &Arc<spitzli_oauth::OAuthService>,
        actor: &spitzli_oauth::McpAuthentication,
        input: Value,
    ) -> Result<Value> {
        if actor.authority.registry_scope() {
            return Err(error(403, "Hosting scope is required."));
        }
        let guard = oauth
            .begin_mcp_authority(&actor.authority)
            .await
            .map_err(|_| error(403, "Hosting authorization is no longer valid."))?;
        self.execute_guarded(
            guard.principal(),
            input,
            HostingCeiling {
                read: true,
                write: guard.can_write(),
            },
            Some(&guard),
        )
        .await
    }
    async fn execute_guarded(
        &self,
        actor: &Principal,
        input: Value,
        ceiling: HostingCeiling,
        guard: Option<&spitzli_oauth::McpGuard>,
    ) -> Result<Value> {
        let cmd = contracts::parse_command(input)?;
        let action = field(&cmd, "action")?;
        let read = action.ends_with(".get")
            || action.ends_with(".list")
            || [
                "apps.logs",
                "apps.inspect",
                "apps.metrics",
                "apps.deletion",
                "apps.storageDeletion",
                "operations.overview",
                "usage.get",
            ]
            .contains(&action);
        if !ceiling.read || (!read && !ceiling.write) {
            return Err(error(403, "Hosting scope is required."));
        }
        let tx = self.begin(actor).await?;
        let actor = &HostingActor::from(actor);
        Self::module(&tx, !read, false).await?;
        let (result, target) = if action.starts_with("apps.") {
            self.apps(&tx, actor, &cmd).await?
        } else {
            self.management(&tx, actor, &cmd).await?
        };
        Self::authorize(
            &tx,
            actor,
            target.project.as_deref(),
            target.customer.as_deref(),
            target.write,
            target.operator,
            target.live,
        )
        .await?;
        if let Some(guard) = guard {
            guard
                .verify()
                .await
                .map_err(|_| error(403, "Hosting authorization is no longer valid."))?;
        }
        tx.commit().await.map_err(db_error)?;
        Ok(result)
    }
    async fn apply_scope(
        &self,
        tx: &DatabaseTransaction,
        audience: &str,
        scope: Value,
    ) -> Result<()> {
        let raw = json!({"v":1,"aud":audience,"exp":now()+60,"scope":scope}).to_string();
        let mut mac = Hmac::<Sha256>::new_from_slice(self.rls_secret.as_bytes())
            .map_err(|_| error(503, "Invalid RLS configuration."))?;
        mac.update(raw.as_bytes());
        one(tx,"SELECT set_config('webdock.rls_context',$1,true),set_config('webdock.rls_signature',$2,true)",vec![raw.into(),hex::encode(mac.finalize().into_bytes()).into()]).await?;
        Ok(())
    }
}
struct Target {
    project: Option<String>,
    customer: Option<String>,
    write: bool,
    operator: bool,
    live: bool,
}
impl Target {
    fn project(id: &str, write: bool, operator: bool, live: bool) -> Self {
        Self {
            project: Some(id.into()),
            customer: None,
            write,
            operator,
            live,
        }
    }
    fn operator(write: bool) -> Self {
        Self {
            project: None,
            customer: None,
            write,
            operator: true,
            live: false,
        }
    }
}
#[path = "agent.rs"]
mod agent;
#[path = "apps.rs"]
mod apps;
#[path = "capacity.rs"]
mod capacity;
#[path = "management.rs"]
mod management;
#[path = "public_address.rs"]
mod public_address;
#[path = "runtime.rs"]
mod runtime;

#[path = "git.rs"]
mod git;

fn is_read_action(action: &str) -> bool {
    matches!(action, "apps.logs" | "apps.inspect" | "apps.metrics")
}
#[path = "reads.rs"]
mod reads;

/// Retained v1 command canonicalization: en-US ICU collation and ECMAScript JSON encoding.
pub fn canonical_command(value: &Value) -> Result<String> {
    management::canonical(value)
}

#[path = "browser_authority.rs"]
mod browser_authority;

#[path = "delivery.rs"]
mod delivery;
pub use delivery::{DeliveryCommand, DeliveryDisposition};

#[path = "completion_deadline.rs"]
mod completion_deadline;
#[path = "recovery.rs"]
mod recovery;

#[cfg(test)]
mod provider_compatibility_tests {
    // These are source/query guards against making customer/Git authorization depend
    // on operator-only cluster visibility. Real RLS/API acceptance remains separate.
    #[test]
    fn customer_and_git_authorization_keep_the_stored_provider_without_cluster_dependency() {
        let source = include_str!("service.rs");
        let authorization = source
            .split("    async fn authorize(")
            .nth(1)
            .unwrap()
            .split("    async fn lock(")
            .next()
            .unwrap();
        assert!(!authorization.contains("project_provider("));
        assert!(!authorization.contains("webdock_auth.hosting_cluster"));
        assert!(authorization.contains("Self::authorize_git("));
        assert!(authorization.contains("LEFT JOIN webdock_auth.hosting_project h"));
    }
    #[test]
    fn project_read_models_do_not_infer_providers_from_rls_hidden_cluster_details() {
        let source = include_str!("management.rs");
        let projects = source
            .split("\"projects.list\" =>")
            .nth(1)
            .unwrap()
            .split("\"limits.get\"")
            .next()
            .unwrap();
        assert!(projects.contains("'provider',h.provider"));
        assert!(!projects.contains("runtime_kind"));
    }
    #[test]
    fn normal_native_project_write_preserves_raw_provider_and_validates_actual_driver() {
        let source = include_str!("management.rs");
        let project = source
            .split("\"projects.create\" | \"projects.update\" =>")
            .nth(1)
            .unwrap()
            .split("\"clusters.register\" =>")
            .next()
            .unwrap();
        assert!(project.contains("Self::cluster_view(tx, cluster).await?"));
        assert!(
            project.contains("(provider == \"docker\") != (state[\"runtimeKind\"] == \"docker\")")
        );
        assert!(project.contains("access.customer.clone().into(),provider.into(),cluster.into()"));
        assert!(!project.contains("canonical_provider"));
    }
    #[test]
    fn actual_runtime_validation_uses_the_existing_scoped_definer() {
        let source = include_str!("capacity.rs");
        let cluster = source
            .split("async fn cluster_view(")
            .nth(1)
            .unwrap()
            .split("async fn check_demand(")
            .next()
            .unwrap();
        assert!(cluster.contains("webdock_rls.hosting_runtime_kind($1)"));
        assert!(!cluster.contains("FROM webdock_auth.hosting_cluster"));
    }
    #[test]
    fn recovery_quota_lookup_keeps_stored_backend_without_a_cluster_projection() {
        let source = include_str!("recovery.rs");
        assert!(
            source
                .contains("SELECT provider FROM webdock_auth.hosting_project WHERE project_id=$1")
        );
        assert!(!source.contains("Self::project_provider("));
    }
    #[test]
    fn registered_runtime_migration_already_supports_raw_docker() {
        let migration = include_str!("../../persistence/migrations/0008_hosting_runtime.sql");
        assert!(migration.contains("CHECK(provider IN ('k3s','docker','vercel'))"));
        assert!(migration.contains(
            "provider IN ('k3s','docker') AND cluster_id IS NOT NULL AND namespace IS NOT NULL"
        ));
        assert!(migration.contains("SECURITY DEFINER"));
        assert!(migration.contains("webdock_rls.capacity_cluster_allowed(cluster_id)"));
    }
}

#[cfg(test)]
mod recovery_db_diagnostic_tests {
    use super::*;
    #[test]
    fn only_sqlstate_and_closed_error_classes_are_observable() {
        assert_eq!(
            recovery_sqlstate(Some("42703".into())),
            Some("42703".into())
        );
        for value in [
            "private-secret",
            "42703\n",
            "postgresql://secret",
            "abcde",
            "",
        ] {
            assert!(recovery_sqlstate(Some(value.into())).is_none());
        }
        assert_eq!(
            recovery_db_observation(&sea_orm::DbErr::Type("secret-must-not-leave".into())),
            ("decode", None)
        );
        let query = sea_orm::DbErr::Query(sea_orm::RuntimeErr::SqlxError(Arc::new(
            sea_orm::sqlx::Error::ColumnNotFound("secret-must-not-leave".into()),
        )));
        assert_eq!(recovery_db_observation(&query), ("query", None));
        assert_eq!(db_error(query).status, 503);
        assert!(
            !db_error(sea_orm::DbErr::Custom("secret-must-not-leave".into()))
                .message
                .contains("secret-must-not-leave")
        );
    }
}
