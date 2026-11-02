use super::capacity::amount;
use super::control::ReceiptScope;
use super::*;
impl DatabaseService {
    pub(super) async fn register_pool(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        authority: &StoredAuthority,
        cmd: &Value,
    ) -> Result<Value> {
        self.authorize_mode(tx, actor, None, None, true, true, authority, true)
            .await?;
        let (hash, prior) = self.replay_control(tx, actor, cmd).await?;
        if let Some(value) = prior {
            return Ok(value);
        }
        let cluster = text(cmd, "clusterID")?;
        self.cluster_lock(tx, cluster).await?;
        let c = self.fresh_cluster(tx, cluster).await?;
        let pool = c["observation"]["capabilities"]["databasePools"]
            .as_array()
            .and_then(|ps| {
                ps.iter().find(|p| {
                    p["node"] == cmd["node"] && p["poolID"] == cmd["poolID"] && p["ready"] == true
                })
            })
            .ok_or_else(|| conflict("Refresh the qualified database pool observation."))?;
        if c["revision"] != cmd["revision"] {
            return Err(conflict("Refresh the qualified database pool observation."));
        }
        let old=optional(tx,"SELECT to_jsonb(p) AS value FROM webdock_auth.hosting_database_pool p WHERE cluster_id=$1 AND pool_id=$2::uuid",vec![cluster.into(),text(cmd,"poolID")?.into()]).await?;
        if old.as_ref().is_some_and(|old| {
            old["node"] != cmd["node"]
                || amount(&old["capacity_bytes"]).ok() != amount(&pool["capacityBytes"]).ok()
        }) {
            return Err(conflict(
                "Changing a registered storage partition requires a separate capacity review.",
            ));
        }
        let others=one(tx,"SELECT jsonb_build_object('bytes',coalesce(sum(capacity_bytes),0)::text) AS value FROM webdock_auth.hosting_database_pool WHERE cluster_id=$1 AND pool_id<>$2::uuid",vec![cluster.into(),text(cmd,"poolID")?.into()]).await?;
        if amount(&c["observation"]["capacity"]["volumeBytes"])?
            + amount(&others["bytes"])?
            + amount(&pool["capacityBytes"])?
            > amount(&c["capacity"]["volumeBytes"])?
        {
            return Err(conflict(
                "Carve the PostgreSQL pool out of the existing shared storage budget first.",
            ));
        }
        exec(tx,"INSERT INTO webdock_auth.hosting_database_pool(cluster_id,node,pool_id,capacity_bytes) VALUES($1,$2,$3::uuid,$4) ON CONFLICT DO NOTHING",vec![cluster.into(),text(cmd,"node")?.into(),text(cmd,"poolID")?.into(),(amount(&pool["capacityBytes"])? as i64).into()]).await?;
        let result = json!({"poolID":cmd["poolID"],"node":cmd["node"],"capacityBytes":pool["capacityBytes"]});
        self.record_control(
            tx,
            actor,
            cmd,
            &hash,
            &result,
            ReceiptScope {
                cluster: Some(cluster),
                ..Default::default()
            },
        )
        .await?;
        self.control_audit(tx, actor, text(cmd, "action")?, cluster)
            .await?;
        Ok(result)
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "Explicit transaction, authority and revision inputs keep this operation boundary auditable."
    )]
    pub(super) async fn enqueue_database(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        authority: &StoredAuthority,
        backend: &str,
        database: Option<&str>,
        action: &str,
        revision: i32,
        set_current: bool,
    ) -> Result<String> {
        Ok(self
            .insert_database_job(
                tx,
                actor,
                authority,
                backend,
                database,
                action,
                revision,
                set_current,
            )
            .await?
            .id)
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "Shared insertion preserves explicit original actor/target/revision while retaining private INSERT facts."
    )]
    pub(super) async fn insert_database_job(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        authority: &StoredAuthority,
        backend: &str,
        database: Option<&str>,
        action: &str,
        revision: i32,
        set_current: bool,
    ) -> Result<QueuedDatabase> {
        if self
            .management
            .as_ref()
            .and_then(|c| c.lease_secret.as_deref())
            .is_none()
        {
            return Err(unavailable());
        }
        let backend = one(
            tx,
            "SELECT to_jsonb(b) AS value FROM webdock_auth.hosting_database_backend b WHERE id=$1",
            vec![backend.into()],
        )
        .await?;
        let target = if let Some(id) = database {
            one(
                tx,
                "SELECT to_jsonb(d) AS value FROM webdock_auth.hosting_database d WHERE id=$1",
                vec![id.into()],
            )
            .await?
        } else {
            json!({})
        };
        let token = token()?;
        let id = self.id().await?;
        let _insert_operation=one(tx,"INSERT INTO webdock_auth.hosting_operation(id,subject,customer_id,project_id,cluster_id,action,idempotency_key,payload_hash,status,desired,target_revision,actor_session) VALUES($1,$2,$3,$4,$5,$6,$7,$8,'queued',$9,$10,$11) RETURNING jsonb_build_object('id',id,'tableOid',tableoid::oid::text,'xmin',xmin::text) AS value",vec![id.clone().into(),actor.user_id.clone().into(),target["customer_id"].as_str().into(),target["project_id"].as_str().into(),text(&backend,"cluster_id")?.into(),format!("databases.{action}").into(),format!("database-job-{token}").into(),hash(&token)?.into(),json!({"databaseVersion":1,"backendID":backend["id"],"databaseID":database,"action":action,"revision":revision,"nativeAuthority":crate::database_authority::DatabaseAuthority::BrowserStored(authority.clone()).to_persisted().map_err(|_|invalid())?}).into(),revision.into(),actor.session_id.clone().into()]).await?;
        let _insert_job=one(tx,"INSERT INTO webdock_auth.hosting_database_job(operation_id,backend_id,database_id,action,revision) VALUES($1,$2,$3,$4,$5) RETURNING jsonb_build_object('id',operation_id,'tableOid',tableoid::oid::text,'xmin',xmin::text) AS value",vec![id.clone().into(),text(&backend,"id")?.into(),database.into(),action.into(),revision.into()]).await?;
        if let Some(database) = database.filter(|_| set_current) {
            exec(
                tx,
                "UPDATE webdock_auth.hosting_database SET operation_id=$2 WHERE id=$1",
                vec![database.into(), id.clone().into()],
            )
            .await?;
        }
        Ok(QueuedDatabase {
            id,
            #[cfg(test)]
            inserts: vec![
                InsertFact::from_returning("operation", &_insert_operation)?,
                InsertFact::from_returning("job", &_insert_job)?,
            ],
        })
    }
    /// Caller holds current offer/plan and scope checks; allocation never enqueues or commits.
    pub(super) async fn allocate_order_target(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        offer: &Value,
        plan: &Value,
        cluster: &str,
    ) -> Result<AllocatedTarget> {
        let customer = text(offer, "customer_id")?;
        let project = text(offer, "project_id")?;
        let p = plan;
        let backend = if p["kind"] == "shared" {
            let candidates=all(tx,"SELECT to_jsonb(b) AS value FROM webdock_auth.hosting_database_backend b WHERE cluster_id=$1 AND kind='shared-pool' AND state='ready' AND observed_revision=revision ORDER BY id FOR UPDATE",vec![cluster.into()]).await?;
            let mut selected = None;
            for candidate in candidates {
                let used = one(
                    tx,
                    "SELECT to_jsonb(s) AS value FROM webdock_rls.capacity_slots($1) s",
                    vec![text(&candidate, "id")?.into()],
                )
                .await?;
                if shared_slot_available(&used, &candidate, p)? {
                    selected = Some(candidate);
                    break;
                }
            }
            selected.ok_or_else(|| conflict("Shared PostgreSQL capacity is unavailable."))?
        } else {
            self.create_backend(tx, cluster, "dedicated", Some(p), None)
                .await?
        };
        let order = self.id().await?;
        let id = self.id().await?;
        let _insert_order=one(tx,"INSERT INTO webdock_auth.hosting_database_order(id,offer_id,customer_id,project_id,plan_revision_id,accepted_by,snapshot) VALUES($1,$2,$3,$4,$5,$6,$7) RETURNING jsonb_build_object('id',id,'tableOid',tableoid::oid::text,'xmin',xmin::text) AS value",vec![order.clone().into(),text(offer,"id")?.into(),customer.into(),project.into(),text(offer,"plan_revision_id")?.into(),actor.user_id.clone().into(),offer["snapshot"].clone().into()]).await?;
        let credentials = DatabaseCredentials {
            application_password: token()?,
            migration_password: token()?,
        };
        let sealed = self
            .cipher
            .seal(&credentials, &format!("database:{id}:1"))
            .map_err(|_| unavailable())?;
        let _insert_database=one(tx,"INSERT INTO webdock_auth.hosting_database(id,customer_id,project_id,order_id,plan_revision_id,backend_id,kind,name,database_name,credentials_encrypted) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) RETURNING jsonb_build_object('id',id,'tableOid',tableoid::oid::text,'xmin',xmin::text) AS value",vec![id.clone().into(),customer.into(),project.into(),order.clone().into(),text(offer,"plan_revision_id")?.into(),text(&backend,"id")?.into(),text(p,"kind")?.into(),text(offer,"name")?.into(),format!("db_{id}").into(),sealed.into()]).await?;
        exec(
            tx,
            "UPDATE webdock_auth.hosting_database_order SET database_id=$2 WHERE id=$1",
            vec![order.clone().into(), id.clone().into()],
        )
        .await?;
        #[cfg(test)]
        let fresh = FreshAllocation::from_inserts(&backend, _insert_order, _insert_database)?;
        Ok(AllocatedTarget {
            #[cfg(test)]
            fresh,
            backend,
            order,
            database: id,
        })
    }
    pub(super) async fn accept_order(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        authority: &StoredAuthority,
        cmd: &Value,
        access: &Value,
        cluster: &str,
    ) -> Result<Value> {
        let (hash, prior) = self.replay_control(tx, actor, cmd).await?;
        let project = text(cmd, "projectID")?;
        let customer = text(access, "customerID")?;
        let offer=one(tx,"SELECT to_jsonb(o) AS value FROM webdock_auth.hosting_database_offer o WHERE id=$1 AND project_id=$2 AND customer_id=$3 FOR UPDATE",vec![text(cmd,"offerID")?.into(),project.into(),customer.into()]).await?;
        let kind = ordinary_offer_kind(&offer["snapshot"])?;
        if let Some(value) = prior {
            return Ok(value);
        }
        if let Some(previous)=optional(tx,"SELECT jsonb_build_object('orderID',o.id,'databaseID',d.id,'operationID',d.operation_id) AS value FROM webdock_auth.hosting_database_order o JOIN webdock_auth.hosting_database d ON d.id=o.database_id WHERE o.offer_id=$1",vec![text(&offer,"id")?.into()]).await?{self.record_control(tx,actor,cmd,&hash,&previous,ReceiptScope{customer:Some(customer),project:Some(project),cluster:Some(cluster)}).await?;return Ok(previous)}
        if kind == OrdinaryOfferKind::Create {
            self.require_new_orders()?;
        }
        let plan=one(tx,"SELECT to_jsonb(p) AS value FROM webdock_auth.hosting_database_plan p WHERE id=$1 FOR SHARE",vec![text(&offer,"plan_revision_id")?.into()]).await?;
        self.plan_available(&plan, customer, access["operator"] == true)?;
        if chrono::DateTime::parse_from_rfc3339(text(&offer, "expires_at")?)
            .map_err(|_| unavailable())?
            <= chrono::Utc::now()
        {
            return Err(conflict(
                "Database offer expired or is no longer available.",
            ));
        }
        self.cluster_lock(tx, cluster).await?;
        self.fresh_cluster(tx, cluster).await?;
        if kind == OrdinaryOfferKind::Upgrade {
            return self
                .accept_upgrade(
                    tx,
                    actor,
                    authority,
                    cmd,
                    &offer,
                    &plan["plan"],
                    cluster,
                    &hash,
                )
                .await;
        }
        let AllocatedTarget {
            #[cfg(test)]
                fresh: _,
            backend,
            order,
            database: id,
        } = self
            .allocate_order_target(tx, actor, &offer, &plan["plan"], cluster)
            .await?;
        let operation = self
            .enqueue_database(
                tx,
                actor,
                authority,
                text(&backend, "id")?,
                Some(&id),
                "create",
                1,
                true,
            )
            .await?;
        let result = json!({"orderID":order,"databaseID":id,"operationID":operation});
        self.record_control(
            tx,
            actor,
            cmd,
            &hash,
            &result,
            ReceiptScope {
                customer: Some(customer),
                project: Some(project),
                cluster: Some(cluster),
            },
        )
        .await?;
        self.control_audit(tx, actor, text(cmd, "action")?, &id)
            .await?;
        Ok(result)
    }
}

pub(super) struct QueuedDatabase {
    pub(super) id: String,
    #[cfg(test)]
    inserts: Vec<InsertFact>,
}
pub(super) struct AllocatedTarget {
    #[cfg(test)]
    pub(super) fresh: Option<FreshAllocation>,
    pub(super) backend: Value,
    pub(super) order: String,
    pub(super) database: String,
}
// Staged facts exist only in the unactivated test build; no client Deserialize.
#[cfg(test)]
pub(super) struct FreshAllocation {
    rows: Vec<InsertFact>,
}
#[cfg(test)]
struct InsertFact {
    kind: &'static str,
    id: String,
    table_oid: String,
    xmin: u32,
}
#[cfg(test)]
impl FreshAllocation {
    fn from_inserts(backend: &Value, order: Value, database: Value) -> Result<Option<Self>> {
        if backend["kind"] != "dedicated" {
            return Ok(None);
        }
        let mut rows = vec![];
        let inserted_backend = json!({"id":backend["id"],"tableOid":backend["_insert"]["tableOid"],"xmin":backend["_insert"]["xmin"]});
        for (kind, value) in [
            ("backend", inserted_backend),
            ("order", order),
            ("database", database),
        ] {
            rows.push(InsertFact::from_returning(kind, &value)?);
        }
        let ports = backend["_insertPorts"].as_array().ok_or_else(unavailable)?;
        if ports.len() != 2 {
            return Err(unavailable());
        }
        for port in ports {
            rows.push(InsertFact::from_returning("port", port)?);
        }
        Ok(Some(Self { rows }))
    }
    pub(super) fn append_job(&mut self, queued: QueuedDatabase) -> String {
        self.rows.extend(queued.inserts);
        queued.id
    }
    pub(super) async fn final_facts(
        &self,
        tx: &DatabaseTransaction,
        backend: &Value,
    ) -> Result<Value> {
        let xid = one(
            tx,
            "SELECT jsonb_build_object('xid',pg_current_xact_id()::text) AS value",
            vec![],
        )
        .await?;
        let top = text(&xid, "xid")?
            .parse::<u64>()
            .map_err(|_| unavailable())?;
        let mut facts = vec![];
        for row in &self.rows {
            let (table, key) = match row.kind {
                "backend" => ("hosting_database_backend", "id"),
                "order" => ("hosting_database_order", "id"),
                "database" => ("hosting_database", "id"),
                "port" => ("hosting_database_port", "port"),
                "operation" => ("hosting_operation", "id"),
                "job" => ("hosting_database_job", "operation_id"),
                _ => return Err(unavailable()),
            };
            let where_clause = if row.kind == "port" {
                "port::text=$1 AND backend_id=$2 AND cluster_id=$3"
            } else if row.kind == "job" {
                "operation_id=$1"
            } else {
                "id=$1"
            };
            let values = if row.kind == "port" {
                vec![
                    row.id.clone().into(),
                    text(backend, "id")?.into(),
                    text(backend, "cluster_id")?.into(),
                ]
            } else {
                vec![row.id.clone().into()]
            };
            let current=one(tx,&format!("SELECT jsonb_build_object('id',{key}::text,'tableOid',tableoid::oid::text,'ctid',ctid::text,'xmin',xmin::text) AS value FROM webdock_auth.{table} WHERE {where_clause}"),values).await?;
            if text(&current, "id")? != row.id || text(&current, "tableOid")? != row.table_oid {
                return Err(unavailable());
            }
            let mut fact = current;
            fact["kind"] = json!(row.kind);
            fact["insertXid8"] =
                json!(super::restore_orders::expand_insert_xid(top, row.xmin)?.to_string());
            facts.push(fact);
        }
        Ok(json!({"topXid8":top.to_string(),"rows":facts}))
    }
}
#[cfg(test)]
impl InsertFact {
    fn from_returning(kind: &'static str, row: &Value) -> Result<Self> {
        let xmin = text(row, "xmin")?
            .parse::<u32>()
            .map_err(|_| unavailable())?;
        if xmin < 3 {
            return Err(unavailable());
        }
        Ok(Self {
            kind,
            id: text(row, "id")?.into(),
            table_oid: text(row, "tableOid")?.into(),
            xmin,
        })
    }
}
#[derive(Debug, PartialEq)]
enum OrdinaryOfferKind {
    Create,
    Upgrade,
}
fn ordinary_offer_kind(snapshot: &Value) -> Result<OrdinaryOfferKind> {
    if !snapshot.is_object() || snapshot.get("purpose").is_some() {
        return Err(conflict(
            "Purpose-specific database offers require their own acceptance.",
        ));
    }
    Ok(if snapshot.get("sourceDatabaseID").is_some() {
        OrdinaryOfferKind::Upgrade
    } else {
        OrdinaryOfferKind::Create
    })
}
fn shared_slot_available(used: &Value, backend: &Value, plan: &Value) -> Result<bool> {
    Ok(amount(&used["count"])? < amount(&backend["max_databases"])?
        && amount(&used["bytes"])? + amount(&plan["logicalStorageAllowanceBytes"])?
            <= amount(&backend["logical_capacity_bytes"])?)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_ordinary_and_upgrade_offers_keep_their_immutable_terms() {
        let ordinary = json!({"kind":"dedicated","monthlyPriceCents":1200,"currency":"EUR","backupStatus":"not-configured","billingStarts":"observed-ready","terms":"Accepted immutable terms"});
        let original = ordinary.clone();
        assert_eq!(
            ordinary_offer_kind(&ordinary).unwrap(),
            OrdinaryOfferKind::Create
        );
        let mut upgrade = ordinary.clone();
        upgrade["sourceDatabaseID"] = json!("101");
        upgrade["sourceRevision"] = json!(2);
        assert_eq!(
            ordinary_offer_kind(&upgrade).unwrap(),
            OrdinaryOfferKind::Upgrade
        );
        assert_eq!(ordinary, original);
        assert_eq!(upgrade["monthlyPriceCents"], original["monthlyPriceCents"]);
        assert_eq!(upgrade["backupStatus"], original["backupStatus"]);
    }
    #[test]
    fn ordinary_acceptance_cannot_reuse_a_restore_or_unknown_purpose_offer() {
        for snapshot in [
            json!({"purpose":"restore","restoreIntent":{"sourceDatabaseID":"101"}}),
            json!({"purpose":"restore","sourceDatabaseID":"101"}),
            json!({"purpose":"unknown"}),
            json!({"purpose":null}),
            Value::Null,
        ] {
            assert!(
                ordinary_offer_kind(&snapshot).is_err(),
                "purpose-specific offers require their own admission"
            );
        }
    }
    #[test]
    fn shared_allocation_requires_both_a_free_slot_and_the_full_retained_allowance() {
        let backend = json!({"max_databases":8,"logical_capacity_bytes":"1000"});
        let plan = json!({"logicalStorageAllowanceBytes":300});
        assert!(
            shared_slot_available(&json!({"count":"7","bytes":"700"}), &backend, &plan).unwrap()
        );
        assert!(!shared_slot_available(&json!({"count":8,"bytes":0}), &backend, &plan).unwrap());
        assert!(!shared_slot_available(&json!({"count":7,"bytes":701}), &backend, &plan).unwrap());
        assert!(shared_slot_available(&json!({"count":7}), &backend, &plan).is_err());
    }
}
