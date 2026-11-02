use crate::{
    RegistryError, Result, contracts,
    entities::{self, audit, customer, project, user},
    query,
};
use sea_orm::{sea_query::Expr, *};
use serde_json::{Value, json};
use std::sync::Arc;
use webdock_identity::{Principal, RlsSigner};

#[async_trait::async_trait]
pub trait Allocator: Send + Sync {
    async fn allocate(&self, tx: &DatabaseTransaction) -> Result<String>;
}
#[async_trait::async_trait]
impl Allocator for webdock_persistence::IdAllocator {
    async fn allocate(&self, _tx: &DatabaseTransaction) -> Result<String> {
        Ok(webdock_persistence::IdAllocator::allocate(self)
            .await?
            .to_string())
    }
}
/// Retained allocator; an installation can inject its coordinated Rust ID allocator.
pub struct DatabaseAllocator;
#[async_trait::async_trait]
impl Allocator for DatabaseAllocator {
    async fn allocate(&self, tx: &DatabaseTransaction) -> Result<String> {
        let row = tx
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT webdock_admin.next_snowflake() AS id",
            ))
            .await?
            .ok_or(RegistryError::Invalid("ID allocation failed."))?;
        let id: String = row.try_get("", "id")?;
        contracts::record_id(&id)?;
        Ok(id)
    }
}
/// Trusted installed-feature guards run on the same locked, authorized transaction.
#[async_trait::async_trait]
pub trait ArchiveGuard: Send + Sync {
    async fn before_archive(&self, tx: &DatabaseTransaction, project_id: &str) -> Result<()>;
}
/// Retained inventory protects existing CMS data even when that feature is disabled.
pub struct RetainedArchiveGuard;
#[async_trait::async_trait]
impl ArchiveGuard for RetainedArchiveGuard {
    async fn before_archive(&self, tx: &DatabaseTransaction, project_id: &str) -> Result<()> {
        let row = tx
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT to_regclass('webdock_admin.cms_instances') IS NOT NULL AS present",
            ))
            .await?
            .ok_or(RegistryError::Invalid("Inventory unavailable."))?;
        if row.try_get::<bool>("","present")? && tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM webdock_admin.cms_instances WHERE project_id=$1 AND status <> 'retired' LIMIT 1",[project_id.into()])).await?.is_some() {return Err(RegistryError::Invalid("Retire the linked CMS before archiving this project."));}
        Ok(())
    }
}
/// Whether the customer has projects (active ones only, if asked) besides its Default project
/// while that one is still empty.
async fn other_projects(tx: &DatabaseTransaction, customer: &str, active: bool) -> Result<bool> {
    let status = if active { "AND status='active'" } else { "" };
    Ok(tx
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("SELECT 1 AS found FROM webdock_admin.projects WHERE customer_id=$1 {status} AND NOT (created_default AND webdock_admin.project_is_empty(id)) LIMIT 1"),
            [customer.into()],
        ))
        .await?
        .is_some())
}
/// The Default project follows its customer through archive and restore; on delete the database
/// removes it while it is empty.
async fn default_projects(tx: &DatabaseTransaction, customer: &str, sql: &str) -> Result<()> {
    tx.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        [customer.into()],
    ))
    .await?;
    Ok(())
}
#[derive(Clone)]
pub struct RegistryService {
    db: DatabaseConnection,
    signer: Arc<RlsSigner>,
    mcp: Option<Arc<webdock_oauth::McpGuard>>,
    allocator: Arc<dyn Allocator>,
    guard: Option<Arc<dyn ArchiveGuard>>,
}
impl RegistryService {
    pub fn new(db: DatabaseConnection, signer: RlsSigner) -> Self {
        Self {
            db,
            signer: Arc::new(signer),
            mcp: None,
            allocator: Arc::new(DatabaseAllocator),
            guard: None,
        }
    }
    pub fn with_allocator(mut self, allocator: Arc<dyn Allocator>) -> Self {
        self.allocator = allocator;
        self
    }
    /// Additional feature guards supplement rather than replace the retained inventory check.
    pub fn with_archive_guard(mut self, guard: Arc<dyn ArchiveGuard>) -> Self {
        self.guard = Some(guard);
        self
    }
    /// Request-owned view: the exact access grant remains checked until commit.
    pub async fn with_mcp_authority(
        &self,
        oauth: &Arc<webdock_oauth::OAuthService>,
        authority: &webdock_oauth::McpAuthority,
    ) -> Result<Self> {
        if !authority.registry_scope() {
            return Err(RegistryError::Forbidden("Registry scope is required."));
        }
        let guard = oauth
            .begin_mcp_authority(authority)
            .await
            .map_err(|_| RegistryError::Forbidden("Resource authorization is no longer valid."))?;
        Ok(Self {
            mcp: Some(Arc::new(guard)),
            ..self.clone()
        })
    }
    async fn verify_mcp(&self, actor: &Principal, write: bool) -> Result<()> {
        let Some(guard) = &self.mcp else {
            return Ok(());
        };
        if guard.principal().user_id != actor.user_id
            || guard.principal().session_id != actor.session_id
            || (write && !guard.can_write())
        {
            return Err(RegistryError::Forbidden("Registry scope is required."));
        }
        guard
            .verify()
            .await
            .map_err(|_| RegistryError::Forbidden("Resource authorization is no longer valid."))
    }
    async fn begin(&self, actor: &Principal, write: bool) -> Result<(DatabaseTransaction, String)> {
        self.verify_mcp(actor, write).await?;
        if !actor.is_operator() {
            return Err(RegistryError::Forbidden(
                "Current operator access is required.",
            ));
        }
        let tx = self.signer.begin(&self.db, actor).await?;
        // Enum names are retained PostgreSQL types. Trusted schemas only, transaction-local.
        tx.execute_unprepared("SET LOCAL search_path = pg_catalog, webdock_admin")
            .await?;
        Self::authorized(&tx).await?;
        let native = user::Entity::find()
            .filter(user::Column::AuthSubject.eq(&actor.user_id))
            .filter(Expr::col(user::Column::Id).eq(Expr::cust("webdock_rls.portal_user()->>'id'")))
            .limit(2)
            .lock_shared()
            .all(&tx)
            .await?;
        if native.len() != 1 {
            return Err(RegistryError::Forbidden("A mapped operator is required."));
        }
        Ok((tx, native[0].id.clone()))
    }
    async fn authorized(tx: &DatabaseTransaction) -> Result<()> {
        let row = tx
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT webdock_rls.operator() AS allowed",
            ))
            .await?
            .ok_or(RegistryError::Forbidden(
                "Current operator access is required.",
            ))?;
        if !row.try_get::<bool>("", "allowed")? {
            return Err(RegistryError::Forbidden("Operator access was revoked."));
        }
        Ok(())
    }
    async fn finish(&self, tx: DatabaseTransaction, value: Value) -> Result<Value> {
        Self::authorized(&tx).await?;
        if let Some(guard) = &self.mcp {
            guard.verify().await.map_err(|_| {
                RegistryError::Forbidden("Resource authorization is no longer valid.")
            })?;
        }
        tx.commit().await?;
        Ok(value)
    }
    async fn audit(
        &self,
        tx: &DatabaseTransaction,
        actor: &str,
        collection: &str,
        before: Option<&Value>,
        after: &Value,
    ) -> Result<()> {
        // Column order matches the retained Drizzle schema, including fields added later.
        let fields = match collection {
            "customers" => vec![
                "name",
                "contactName",
                "contactEmail",
                "notes",
                "status",
                "customerType",
                "firstName",
                "lastName",
                "companyName",
                "phone",
                "addressLine1",
                "addressLine2",
                "postalCode",
                "city",
                "region",
                "country",
            ],
            _ => vec![
                "name",
                "customer",
                "url",
                "repositoryURL",
                "notes",
                "status",
            ],
        };
        let changed = fields
            .into_iter()
            .filter(|key| before.and_then(|v| v.get(*key)) != after.get(*key))
            .collect::<Vec<_>>()
            .join(", ");
        audit::ActiveModel {
            id: Set(self.allocator.allocate(tx).await?),
            actor: Set(actor.to_owned()),
            action: Set(if before.is_some() {
                entities::AuditAction::Update
            } else {
                entities::AuditAction::Create
            }),
            target_collection: Set(collection.to_owned()),
            target_id: Set(after["id"].as_str().unwrap().to_owned()),
            summary: Set(format!(
                "{} {}",
                if before.is_some() {
                    "Updated"
                } else {
                    "Created"
                },
                after["name"].as_str().unwrap_or(collection)
            )),
            changed_fields: Set(Some(changed)),
            ..Default::default()
        }
        .insert(tx)
        .await?;
        Ok(())
    }
    pub async fn save_customer(
        &self,
        actor: &Principal,
        key: Option<&str>,
        input: Value,
    ) -> Result<Value> {
        if let Some(key) = key {
            contracts::record_id(key)?;
        }
        let data = contracts::customer_input(input)?;
        let (tx, local) = self.begin(actor, true).await?;
        let old = match key {
            Some(key) => Some(
                customer::Entity::find_by_id(key)
                    .lock_exclusive()
                    .one(&tx)
                    .await?
                    .ok_or(RegistryError::NotFound("Customer unavailable."))?,
            ),
            None => None,
        };
        let before = old.map(query::encode).transpose()?;
        let mut updated = match &before {
            Some(old) => old.clone(),
            None => {
                json!({"id":self.allocator.allocate(&tx).await?,"status":"active","customerType":"company","createdAt":chrono::Utc::now(),"updatedAt":chrono::Utc::now()})
            }
        };
        updated
            .as_object_mut()
            .unwrap()
            .extend(data.as_object().unwrap().clone());
        updated["updatedAt"] = json!(chrono::Utc::now());
        // The retained customer trigger owns the tenant mapping; pass it a separately
        // committed allocation so the implicit organization uses the same Rust generator.
        // Bind to this customer and clear after the statement to avoid pooled-session state.
        let organization_id = self.allocator.allocate(&tx).await?;
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT set_config('webdock.organization_allocation',$1,true)",
            [
                json!({"customer_id":updated["id"],"organization_id":organization_id})
                    .to_string()
                    .into(),
            ],
        ))
        .await?;
        let model = customer::ActiveModel::from_json(updated)?;
        let after = query::encode(if key.is_some() {
            model.update(&tx).await?
        } else {
            model.insert(&tx).await?
        })?;
        tx.execute_unprepared("SELECT set_config('webdock.organization_allocation','',true)")
            .await?;
        self.audit(&tx, &local, "customers", before.as_ref(), &after)
            .await?;
        self.finish(tx, after).await
    }
    pub async fn save_project(
        &self,
        actor: &Principal,
        key: Option<&str>,
        input: Value,
    ) -> Result<Value> {
        if let Some(key) = key {
            contracts::record_id(key)?;
        }
        let data = contracts::project_input(input)?;
        let (tx, local) = self.begin(actor, true).await?;
        let old = match key {
            Some(key) => Some(
                project::Entity::find_by_id(key)
                    .lock_exclusive()
                    .one(&tx)
                    .await?
                    .ok_or(RegistryError::NotFound("Project unavailable."))?,
            ),
            None => None,
        };
        let owner = data["customer"].as_str().unwrap();
        if old.as_ref().is_some_and(|old| old.customer != owner) {
            return Err(RegistryError::Invalid(
                "Project ownership cannot be reassigned here.",
            ));
        }
        let parent = customer::Entity::find_by_id(owner)
            .lock_shared()
            .one(&tx)
            .await?;
        if parent.is_none()
            || (parent.as_ref().unwrap().status != entities::CustomerStatus::Active
                && !old
                    .as_ref()
                    .is_some_and(|o| o.status == entities::ProjectStatus::Archived))
        {
            return Err(RegistryError::Invalid("Choose an active customer."));
        }
        let before = old.map(query::encode).transpose()?;
        let mut updated = match &before {
            Some(old) => old.clone(),
            None => {
                json!({"id":self.allocator.allocate(&tx).await?,"status":"active","createdAt":chrono::Utc::now(),"updatedAt":chrono::Utc::now()})
            }
        };
        updated
            .as_object_mut()
            .unwrap()
            .extend(data.as_object().unwrap().clone());
        updated["updatedAt"] = json!(chrono::Utc::now());
        let model = project::ActiveModel::from_json(updated)?;
        let after = query::encode(if key.is_some() {
            model.update(&tx).await?
        } else {
            model.insert(&tx).await?
        })?;
        self.audit(&tx, &local, "projects", before.as_ref(), &after)
            .await?;
        self.finish(tx, after).await
    }
    pub async fn get(&self, actor: &Principal, collection: &str, key: &str) -> Result<Value> {
        contracts::record_id(key)?;
        public_collection(collection)?;
        let (tx, _) = self.begin(actor, false).await?;
        let value = record(&tx, collection, key)
            .await?
            .ok_or(RegistryError::NotFound("Record unavailable."))?;
        self.finish(tx, value).await
    }
    pub async fn list(&self, actor: &Principal, input: Value) -> Result<Value> {
        let input = contracts::ListInput::parse(input)?;
        public_collection(&input.collection)?;
        let (tx, _) = self.begin(actor, false).await?;
        let value = match input.collection.as_str() {
            "customers" => query::list::<customer::Entity>(&tx, &input).await?,
            "projects" => query::list::<project::Entity>(&tx, &input).await?,
            _ => query::list::<audit::Entity>(&tx, &input).await?,
        };
        self.finish(tx, value).await
    }
    pub async fn archive(
        &self,
        actor: &Principal,
        collection: &str,
        key: &str,
        archived: bool,
    ) -> Result<Value> {
        contracts::record_id(key)?;
        if !["customers", "projects"].contains(&collection) {
            return Err(RegistryError::Invalid("Invalid collection."));
        }
        let (tx, local) = self.begin(actor, true).await?;
        let before = match collection {
            "customers" => query::encode(
                customer::Entity::find_by_id(key)
                    .lock_exclusive()
                    .one(&tx)
                    .await?
                    .ok_or(RegistryError::NotFound("Record unavailable."))?,
            )?,
            _ => query::encode(
                project::Entity::find_by_id(key)
                    .lock_exclusive()
                    .one(&tx)
                    .await?
                    .ok_or(RegistryError::NotFound("Record unavailable."))?,
            )?,
        };
        if archived && collection == "customers" {
            if other_projects(&tx, key, true).await? {
                return Err(RegistryError::Invalid(
                    "Archive this customer’s active projects first.",
                ));
            }
            default_projects(&tx, key, "UPDATE webdock_admin.projects SET status='archived',updated_at=now() WHERE customer_id=$1 AND created_default AND status='active'").await?;
        } else if collection == "customers" {
            default_projects(&tx, key, "UPDATE webdock_admin.projects SET status='active',updated_at=now() WHERE customer_id=$1 AND created_default AND status='archived'").await?;
        } else if archived {
            RetainedArchiveGuard.before_archive(&tx, key).await?;
            if let Some(guard) = &self.guard {
                guard.before_archive(&tx, key).await?;
            }
        } else if collection == "projects" {
            let parent = customer::Entity::find_by_id(before["customer"].as_str().unwrap())
                .lock_shared()
                .one(&tx)
                .await?;
            if !parent.is_some_and(|p| p.status == entities::CustomerStatus::Active) {
                return Err(RegistryError::Invalid("Choose an active customer."));
            }
        }
        let mut after = before.clone();
        after["status"] = json!(if archived { "archived" } else { "active" });
        after["updatedAt"] = json!(chrono::Utc::now());
        let after = if collection == "customers" {
            query::encode(customer::ActiveModel::from_json(after)?.update(&tx).await?)?
        } else {
            query::encode(project::ActiveModel::from_json(after)?.update(&tx).await?)?
        };
        self.audit(&tx, &local, collection, Some(&before), &after)
            .await?;
        self.finish(tx, after).await
    }
    /// Permanently removes an archived customer without projects. The tenant link is released
    /// by the customer delete trigger; hosting, billing, mail and database records keep their
    /// restricting foreign keys, so a customer that still owns any of them is refused.
    pub async fn delete_customer(&self, actor: &Principal, key: &str) -> Result<Value> {
        contracts::record_id(key)?;
        let (tx, local) = self.begin(actor, true).await?;
        let old = customer::Entity::find_by_id(key)
            .lock_exclusive()
            .one(&tx)
            .await?
            .ok_or(RegistryError::NotFound("Customer unavailable."))?;
        if old.status != entities::CustomerStatus::Archived {
            return Err(RegistryError::Conflict(
                "Archive this customer before deleting it.",
            ));
        }
        if other_projects(&tx, key, false).await? {
            return Err(RegistryError::Conflict(
                "This customer still has projects. Delete or move them first.",
            ));
        }
        customer::Entity::delete_by_id(key)
            .exec(&tx)
            .await
            .map_err(|error| {
                if matches!(
                    error.sql_err(),
                    Some(SqlErr::ForeignKeyConstraintViolation(_))
                ) {
                    RegistryError::Conflict(
                        "Hosting, billing, mail, database or preview records still belong to this customer.",
                    )
                } else {
                    error.into()
                }
            })?;
        audit::ActiveModel {
            id: Set(self.allocator.allocate(&tx).await?),
            actor: Set(local),
            action: Set(entities::AuditAction::Delete),
            target_collection: Set("customers".into()),
            target_id: Set(key.into()),
            summary: Set(format!("Deleted {}", old.name)),
            changed_fields: Set(None),
            ..Default::default()
        }
        .insert(&tx)
        .await?;
        self.finish(tx, json!({"id":key,"deleted":true})).await
    }
    pub async fn patch_repository(&self, actor: &Principal, key: &str, url: &str) -> Result<Value> {
        contracts::record_id(key)?;
        contracts::https_url(&json!(url))?;
        let (tx, local) = self.begin(actor, true).await?;
        let old = project::Entity::find_by_id(key)
            .lock_exclusive()
            .one(&tx)
            .await?
            .ok_or(RegistryError::NotFound("Project unavailable."))?;
        if old.status != entities::ProjectStatus::Active {
            return Err(RegistryError::Conflict(
                "Restore this project before linking a repository.",
            ));
        }
        let before = query::encode(old.clone())?;
        let mut model = old.into_active_model();
        model.repository_url = Set(Some(url.to_owned()));
        model.updated_at = Set(chrono::Utc::now().into());
        let after = query::encode(model.update(&tx).await?)?;
        self.audit(&tx, &local, "projects", Some(&before), &after)
            .await?;
        self.finish(tx, after).await
    }
    pub async fn operator(&self, actor: &Principal) -> Result<Value> {
        let (tx, local) = self.begin(actor, false).await?;
        let row = user::Entity::find_by_id(local)
            .one(&tx)
            .await?
            .ok_or(RegistryError::Forbidden("A mapped operator is required."))?;
        let mut value = query::encode(row)?;
        value["role"] = json!("operator");
        value["collection"] = json!("users");
        self.finish(tx, value).await
    }
    pub async fn query(&self, actor: &Principal, mode: &str, input: Value) -> Result<Value> {
        let raw = input
            .as_object()
            .ok_or(RegistryError::Invalid("Invalid registry operation."))?;
        if !["find", "findByID", "count"].contains(&mode)
            || raw.keys().any(|k| {
                ![
                    "collection",
                    "where",
                    "id",
                    "page",
                    "limit",
                    "sort",
                    "depth",
                    "pagination",
                    "select",
                    "disableErrors",
                ]
                .contains(&k.as_str())
            })
        {
            return Err(RegistryError::Invalid("Invalid registry operation."));
        }
        let collection = input["collection"]
            .as_str()
            .ok_or(RegistryError::Invalid("Invalid collection."))?;
        if collection != "users" {
            public_collection(collection)?;
        }
        let depth = query::number(&input, "depth", 1, 2)?;
        // Validate all pagination even for count, as the compatibility contract requires.
        query::number(&input, "page", 1, 10000)?;
        query::number(&input, "limit", 20, 1000)?;
        let (tx, _) = self.begin(actor, false).await?;
        let mut value = match collection {
            "customers" => query::query::<customer::Entity>(&tx, mode, &input).await?,
            "projects" => query::query::<project::Entity>(&tx, mode, &input).await?,
            "audit-events" => query::query::<audit::Entity>(&tx, mode, &input).await?,
            _ => query::query::<user::Entity>(&tx, mode, &input).await?,
        };
        if depth > 0 && mode != "count" {
            if mode == "findByID" {
                populate(&tx, collection, &mut value).await?;
            } else if let Some(docs) = value["docs"].as_array_mut() {
                for doc in docs {
                    populate(&tx, collection, doc).await?;
                }
            }
        }
        self.finish(tx, value).await
    }
}
fn public_collection(collection: &str) -> Result<()> {
    match collection {
        "customers" | "projects" | "audit-events" => Ok(()),
        "users" => Err(RegistryError::Invalid("Invalid collection.")),
        _ => Err(RegistryError::NotFound("Registry collection unavailable.")),
    }
}
async fn record(tx: &DatabaseTransaction, collection: &str, key: &str) -> Result<Option<Value>> {
    match collection {
        "customers" => customer::Entity::find_by_id(key)
            .one(tx)
            .await?
            .map(query::encode)
            .transpose(),
        "projects" => project::Entity::find_by_id(key)
            .one(tx)
            .await?
            .map(query::encode)
            .transpose(),
        "audit-events" => audit::Entity::find_by_id(key)
            .one(tx)
            .await?
            .map(query::encode)
            .transpose(),
        "users" => user::Entity::find_by_id(key)
            .one(tx)
            .await?
            .map(query::encode)
            .transpose(),
        _ => Err(RegistryError::NotFound("Registry collection unavailable.")),
    }
}
async fn populate(tx: &DatabaseTransaction, collection: &str, value: &mut Value) -> Result<()> {
    let (field, parent) = match collection {
        "projects" => ("customer", "customers"),
        "audit-events" => ("actor", "users"),
        _ => return Ok(()),
    };
    if let Some(key) = value.get(field).and_then(Value::as_str)
        && let Some(row) = record(tx, parent, key).await?
    {
        value[field] = row;
    }
    Ok(())
}
