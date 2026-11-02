use super::capacity::amount;
use super::control::ReceiptScope;
use super::*;

impl DatabaseService {
    #[expect(
        clippy::too_many_arguments,
        reason = "Explicit transaction, authority and revision inputs keep this operation boundary auditable."
    )]
    pub(super) async fn accept_upgrade(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        authority: &StoredAuthority,
        cmd: &Value,
        offer: &Value,
        plan: &Value,
        cluster: &str,
        receipt_hash: &str,
    ) -> Result<Value> {
        if cmd["confirmInterruption"] != true {
            return Err(conflict(
                "Confirm the maintenance interruption before accepting this upgrade.",
            ));
        }
        let d = optional(tx,"SELECT to_jsonb(d)||jsonb_build_object('plan',p.plan) AS value FROM webdock_auth.hosting_database d JOIN webdock_auth.hosting_database_plan p ON p.id=d.plan_revision_id WHERE d.id=$1 AND d.project_id=$2 FOR UPDATE OF d",vec![text(&offer["snapshot"],"sourceDatabaseID")?.into(),text(cmd,"projectID")?.into()]).await?.ok_or_else(||conflict("Database changed. Request a new upgrade offer."))?;
        if d["revision"] != offer["snapshot"]["sourceRevision"]
            || d["state"] != "ready"
            || !d["pending_order_id"].is_null()
        {
            return Err(conflict("Database changed. Request a new upgrade offer."));
        }
        let database = text(&d, "id")?;
        self.protect_database(database, "upgrade")?;
        if optional(tx,"SELECT to_jsonb(o) AS value FROM webdock_auth.hosting_database_order o WHERE id=$1 AND cancellation_requested_at IS NOT NULL",vec![text(&d,"order_id")?.into()]).await?.is_some(){return Err(conflict("A cancelled database cannot be upgraded."));}
        let backend=one(tx,"SELECT to_jsonb(b) AS value FROM webdock_auth.hosting_database_backend b WHERE id=$1 FOR UPDATE",vec![text(&d,"backend_id")?.into()]).await?;
        let old = &d["plan"];
        let migration = plan["kind"] != d["kind"] || plan["volumeBytes"] != old["volumeBytes"];
        let target = if migration {
            if plan["kind"] != "dedicated"
                || (old["kind"] == "dedicated"
                    && (amount(&plan["volumeBytes"])? <= amount(&old["volumeBytes"])?
                        || amount(&plan["cpuMillicores"])? < amount(&old["cpuMillicores"])?
                        || amount(&plan["memoryBytes"])? < amount(&old["memoryBytes"])?))
            {
                return Err(conflict(
                    "Only Shared to Dedicated or a larger dedicated filesystem is supported.",
                ));
            }
            let seen = d["usage"]["observedAt"]
                .as_str()
                .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok());
            if backend["cnpg_uid"].as_str().is_none_or(str::is_empty)
                || seen.is_none_or(|t| {
                    chrono::Utc::now()
                        .signed_duration_since(t)
                        .num_milliseconds()
                        > 90000
                })
                || amount(&d["usage"]["logicalBytes"])? > amount(&plan["volumeBytes"])? * 7 / 10
            {
                return Err(conflict(
                    "A fresh source-size observation and sufficient migration headroom are required.",
                ));
            }
            self.create_backend(tx, cluster, "dedicated", Some(plan), None)
                .await?
        } else {
            if plan["kind"] == "shared" {
                if amount(&plan["logicalStorageAllowanceBytes"])?
                    < amount(&old["logicalStorageAllowanceBytes"])?
                {
                    return Err(conflict("Storage allowance cannot shrink."));
                }
                let used = one(
                    tx,
                    "SELECT to_jsonb(s) AS value FROM webdock_rls.capacity_slots($1,$2) s",
                    vec![text(&backend, "id")?.into(), database.into()],
                )
                .await?;
                if amount(&used["bytes"])? + amount(&plan["logicalStorageAllowanceBytes"])?
                    > amount(&backend["logical_capacity_bytes"])?
                {
                    return Err(conflict(
                        "Shared storage allowance capacity is unavailable.",
                    ));
                }
            } else {
                self.resize_backend(tx, cluster, &backend, old, plan)
                    .await?;
            }
            backend.clone()
        };
        let order = self.id().await?;
        exec(tx,"INSERT INTO webdock_auth.hosting_database_order(id,offer_id,customer_id,project_id,plan_revision_id,accepted_by,snapshot,database_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8)",vec![order.clone().into(),text(offer,"id")?.into(),text(&d,"customer_id")?.into(),text(&d,"project_id")?.into(),text(offer,"plan_revision_id")?.into(),actor.user_id.clone().into(),offer["snapshot"].clone().into(),database.into()]).await?;
        if migration {
            let id = self.id().await?;
            let transfer = self
                .cipher
                .seal(
                    &json!({"password":token()?}),
                    &format!("database-migration:{id}"),
                )
                .map_err(|_| unavailable())?;
            exec(tx,"INSERT INTO webdock_auth.hosting_database_migration(id,database_id,source_backend_id,target_backend_id,target_order_id,source_order_id,source_plan_revision_id,source_credential_revision,source_credentials_encrypted,transfer_credentials_encrypted,source_allowance_bytes) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",vec![id.into(),database.into(),text(&backend,"id")?.into(),text(&target,"id")?.into(),order.clone().into(),text(&d,"order_id")?.into(),text(&d,"plan_revision_id")?.into(),(amount(&d["credential_revision"])? as i32).into(),text(&d,"credentials_encrypted")?.into(),transfer.into(),(old["logicalStorageAllowanceBytes"].as_i64().unwrap_or(0)).into()]).await?;
            let revision = amount(&d["credential_revision"])? + 1;
            let credentials = DatabaseCredentials {
                application_password: token()?,
                migration_password: token()?,
            };
            let sealed = self
                .cipher
                .seal(&credentials, &format!("database:{database}:{revision}"))
                .map_err(|_| unavailable())?;
            exec(tx,"UPDATE webdock_auth.hosting_database SET pending_plan_revision_id=$2,pending_order_id=$3,pending_backend_id=$4,credential_revision=$5,credentials_encrypted=$6,revision=revision+1,state='updating' WHERE id=$1",vec![database.into(),text(offer,"plan_revision_id")?.into(),order.clone().into(),text(&target,"id")?.into(),(revision as i32).into(),sealed.into()]).await?;
        } else {
            exec(tx,"UPDATE webdock_auth.hosting_database SET pending_plan_revision_id=$2,pending_order_id=$3,revision=revision+1,state='updating' WHERE id=$1",vec![database.into(),text(offer,"plan_revision_id")?.into(),order.clone().into()]).await?;
        }
        let operation = self
            .enqueue_database(
                tx,
                actor,
                authority,
                text(&target, "id")?,
                Some(database),
                if migration {
                    "prepareMigration"
                } else {
                    "resize"
                },
                (amount(&d["revision"])? + 1) as i32,
                true,
            )
            .await?;
        if migration {
            exec(tx,"UPDATE webdock_auth.hosting_database_job SET source_backend_id=$2 WHERE operation_id=$1",vec![operation.clone().into(),text(&backend,"id")?.into()]).await?;
        }
        let result = json!({"orderID":order,"databaseID":database,"operationID":operation});
        self.record_control(
            tx,
            actor,
            cmd,
            receipt_hash,
            &result,
            ReceiptScope {
                customer: Some(text(&d, "customer_id")?),
                project: Some(text(&d, "project_id")?),
                cluster: Some(cluster),
            },
        )
        .await?;
        self.control_audit(
            tx,
            actor,
            if migration {
                "databases.migrate"
            } else {
                "databases.resize"
            },
            database,
        )
        .await?;
        Ok(result)
    }
    async fn resize_backend(
        &self,
        tx: &DatabaseTransaction,
        cluster: &str,
        backend: &Value,
        old: &Value,
        plan: &Value,
    ) -> Result<()> {
        if plan["volumeBytes"] != old["volumeBytes"]
            || plan["volumeBytes"] != backend["profile"]["volumeBytes"]
        {
            return Err(conflict(
                "Changing the fixed filesystem requires a database migration.",
            ));
        }
        let c = self.fresh_cluster(tx, cluster).await?;
        let apps = self.capacity_demand(tx, cluster, "apps").await?;
        let native = self.capacity_demand(tx, cluster, "native").await?;
        let pool = c["observation"]["capabilities"]["databasePools"]
            .as_array()
            .and_then(|ps| {
                ps.iter().find(|p| {
                    p["node"] == backend["node"]
                        && p["poolID"] == backend["pool_id"]
                        && p["ready"] == true
                })
            })
            .ok_or_else(|| {
                conflict("Database placement requires a fresh qualified storage node.")
            })?;
        let node = c["observation"]["nodes"]
            .as_array()
            .and_then(|ns| ns.iter().find(|n| n["id"] == backend["node"]))
            .ok_or_else(unavailable)?;
        let pools = all(
            tx,
            "SELECT to_jsonb(p) AS value FROM webdock_rls.capacity_pools($1) p",
            vec![cluster.into()],
        )
        .await?;
        let aggregate = pools
            .iter()
            .find(|p| p["node"] == backend["node"] && p["pool_id"] == backend["pool_id"])
            .ok_or_else(|| conflict("Database placement capacity is unavailable."))?;
        let mut demand = backend["demand"].clone();
        for (key, requested, known, held) in [
            (
                "cpuMillicores",
                "requestedCpuMillicores",
                "known_cpu",
                "held_cpu",
            ),
            (
                "memoryBytes",
                "requestedMemoryBytes",
                "known_memory",
                "held_memory",
            ),
        ] {
            let delta = amount(&plan[key])?
                .checked_sub(amount(&old[key])?)
                .ok_or_else(|| conflict("Instance resources cannot shrink."))?;
            if amount(&apps[key])? + amount(&native[key])? + delta > amount(&c["capacity"][key])? {
                return Err(conflict(
                    "Cluster capacity is insufficient for this upgrade.",
                ));
            }
            if amount(&pool[requested])?.saturating_sub(amount(&aggregate[known])?)
                + amount(&aggregate[held])?
                + amount(&apps[key])?
                + delta
                > amount(&node[key])? * 4 / 5
            {
                return Err(conflict(
                    "Database node capacity is insufficient for this upgrade.",
                ));
            }
            demand[key] = json!(amount(&demand[key])? + delta);
        }
        let profile = json!({"cpuMillicores":plan["cpuMillicores"],"memoryBytes":plan["memoryBytes"],"volumeBytes":plan["volumeBytes"]});
        exec(tx,"UPDATE webdock_auth.hosting_database_backend SET demand=$2,profile=$3,revision=revision+1,state='updating' WHERE id=$1",vec![text(backend,"id")?.into(),demand.into(),profile.into()]).await?;
        one(
            tx,
            "SELECT webdock_rls.resize_database_backup_policy($1,$2,$3) AS value",
            vec![
                text(backend, "id")?.into(),
                number(backend, "revision")?.into(),
                (number(backend, "revision")? + 1).into(),
            ],
        )
        .await?;
        Ok(())
    }
}
