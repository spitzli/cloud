use super::*;
const DIMENSIONS: [&str; 7] = [
    "apps",
    "cpuMillicores",
    "memoryBytes",
    "volumeBytes",
    "ephemeralBytes",
    "replicasPerApp",
    "concurrentDeployments",
];
pub(super) fn amount(v: &Value) -> Result<u64> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .filter(|v| *v <= 9_007_199_254_740_991)
        .ok_or_else(|| conflict("Database capacity observation is invalid."))
}
pub(super) fn fits(
    pool: &Value,
    aggregate: &Value,
    node: &Value,
    apps: &Value,
    demand: &Value,
) -> Result<bool> {
    let held_cpu = amount(&aggregate["held_cpu"])?;
    let held_memory = amount(&aggregate["held_memory"])?;
    let held_volume = amount(&aggregate["held_volume"])?;
    let unknown_cpu =
        amount(&pool["requestedCpuMillicores"])?.saturating_sub(amount(&aggregate["known_cpu"])?);
    let unknown_memory =
        amount(&pool["requestedMemoryBytes"])?.saturating_sub(amount(&aggregate["known_memory"])?);
    let allocated = amount(&aggregate["allocated"])?;
    let unknown_volume =
        amount(pool.get("allocatedBytes").unwrap_or(&json!(0)))?.saturating_sub(allocated);
    let pending = held_volume.saturating_sub(allocated);
    Ok(
        unknown_cpu
            + held_cpu
            + amount(&apps["cpuMillicores"])?
            + amount(&demand["cpuMillicores"])?
            <= amount(&node["cpuMillicores"])? * 4 / 5
            && unknown_memory
                + held_memory
                + amount(&apps["memoryBytes"])?
                + amount(&demand["memoryBytes"])?
                <= amount(&node["memoryBytes"])? * 4 / 5
            && held_volume + unknown_volume + amount(&demand["volumeBytes"])?
                <= amount(&pool["capacityBytes"])?
            && amount(&pool["availableBytes"])? >= pending + amount(&demand["volumeBytes"])?,
    )
}
impl DatabaseService {
    pub(super) async fn cluster_lock(&self, tx: &DatabaseTransaction, id: &str) -> Result<()> {
        tx.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
            [format!("hosting-cluster:{id}").into()],
        ))
        .await?;
        Ok(())
    }
    pub(super) async fn fresh_cluster(&self, tx: &DatabaseTransaction, id: &str) -> Result<Value> {
        let c = one(
            tx,
            "SELECT webdock_rls.cluster_snapshot($1) AS value",
            vec![id.into()],
        )
        .await?;
        let seen = c["last_seen"]
            .as_str()
            .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok());
        if c["ownership"] != "platform"
            || c["verified"] != true
            || c["revoked"] == true
            || seen.is_none_or(|t| {
                chrono::Utc::now()
                    .signed_duration_since(t)
                    .num_milliseconds()
                    >= 90000
            })
            || c["observation"]["capabilities"]["databaseVersion"] != 1
        {
            return Err(conflict("A fresh qualified PostgreSQL agent is required."));
        }
        Ok(c)
    }
    pub(super) async fn capacity_demand(
        &self,
        tx: &DatabaseTransaction,
        cluster: &str,
        kind: &str,
    ) -> Result<Value> {
        let value = one(
            tx,
            "SELECT webdock_rls.capacity_demand($1,$2,NULL) AS value",
            vec![cluster.into(), kind.into()],
        )
        .await?;
        if value.as_object().is_none_or(|v| v.len() != 7) {
            return Err(unavailable());
        }
        for key in DIMENSIONS {
            amount(&value[key])?;
        }
        Ok(value)
    }
    pub(super) async fn create_backend(
        &self,
        tx: &DatabaseTransaction,
        cluster: &str,
        kind: &str,
        plan: Option<&Value>,
        selected: Option<(&str, &str)>,
    ) -> Result<Value> {
        self.cluster_lock(tx, cluster).await?;
        let c = self.fresh_cluster(tx, cluster).await?;
        let shared = kind == "shared-pool";
        let profile = if shared {
            json!({"cpuMillicores":1000,"memoryBytes":2147483648_u64,"volumeBytes":32212254720_u64})
        } else {
            let p = plan.ok_or_else(invalid)?;
            crate::management::validate_plan(p)?;
            json!({"cpuMillicores":p["cpuMillicores"],"memoryBytes":p["memoryBytes"],"volumeBytes":p["volumeBytes"]})
        };
        let demand = json!({"apps":2,"cpuMillicores":amount(&profile["cpuMillicores"])?+200,"memoryBytes":amount(&profile["memoryBytes"])?+536870912,"volumeBytes":profile["volumeBytes"],"ephemeralBytes":536870912,"replicasPerApp":1,"concurrentDeployments":1});
        let apps = self.capacity_demand(tx, cluster, "apps").await?;
        let native = self.capacity_demand(tx, cluster, "native").await?;
        for key in DIMENSIONS {
            let used = if key == "replicasPerApp" {
                amount(&apps[key])?
                    .max(amount(&native[key])?)
                    .max(amount(&demand[key])?)
            } else {
                amount(&apps[key])?
                    + amount(&native[key])?
                    + if key == "volumeBytes" {
                        0
                    } else {
                        amount(&demand[key])?
                    }
            };
            if c["capacity"][key].is_null() || used > amount(&c["capacity"][key])? {
                return Err(conflict(
                    "Cluster capacity is insufficient for this database.",
                ));
            }
        }
        let registered = all(
            tx,
            "SELECT to_jsonb(p) AS value FROM webdock_rls.capacity_pools($1) p",
            vec![cluster.into()],
        )
        .await?;
        let pools = c["observation"]["capabilities"]["databasePools"]
            .as_array()
            .ok_or_else(|| conflict("No qualified database volume pool is available."))?;
        let nodes = c["observation"]["nodes"]
            .as_array()
            .ok_or_else(unavailable)?;
        let mut choice = None;
        for pool in pools {
            if pool["ready"] != true
                || selected.is_some_and(|(node, id)| pool["node"] != node || pool["poolID"] != id)
            {
                continue;
            }
            let Some(aggregate) = registered
                .iter()
                .find(|p| p["node"] == pool["node"] && p["pool_id"] == pool["poolID"])
            else {
                continue;
            };
            if amount(&aggregate["capacity_bytes"])? != amount(&pool["capacityBytes"])? {
                continue;
            }
            let Some(node) = nodes.iter().find(|n| n["id"] == pool["node"]) else {
                continue;
            };
            if fits(pool, aggregate, node, &apps, &demand)? {
                choice = Some(pool);
                break;
            }
        }
        let pool = choice.ok_or_else(|| {
            conflict("No qualified database node and volume pool has sufficient capacity.")
        })?;
        let suffix = self
            .management
            .as_ref()
            .and_then(|m| m.endpoint_domain.as_deref())
            .ok_or_else(unavailable)?;
        let labels = suffix.split('.').collect::<Vec<_>>();
        if suffix.len() > 220
            || labels.len() < 2
            || labels.iter().any(|l| {
                l.is_empty()
                    || l.len() > 63
                    || l.starts_with('-')
                    || l.ends_with('-')
                    || !l
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            })
            || labels
                .last()
                .is_none_or(|l| l.len() < 2 || !l.bytes().all(|b| b.is_ascii_lowercase()))
        {
            return Err(unavailable());
        }
        let mut used = std::collections::HashSet::new();
        if let Some(ports) = c["observation"]["capabilities"]["databaseNodePorts"].as_array() {
            for port in ports {
                used.insert(amount(port)?);
            }
        }
        for row in all(tx,"SELECT jsonb_build_object('port',port) AS value FROM webdock_auth.hosting_database_port WHERE cluster_id=$1",vec![cluster.into()]).await?{used.insert(amount(&row["port"])?);}
        let ports = (32000_u64..=32199)
            .filter(|port| !used.contains(port))
            .take(2)
            .collect::<Vec<_>>();
        if ports.len() != 2 {
            return Err(conflict("Database endpoint capacity is unavailable."));
        }
        let id = self.id().await?;
        let returning = "to_jsonb(hosting_database_backend)||jsonb_build_object('_insert',jsonb_build_object('tableOid',tableoid::oid::text,'xmin',xmin::text))";
        let backend=one(tx,&format!("INSERT INTO webdock_auth.hosting_database_backend(id,cluster_id,kind,node,pool_id,demand,profile,max_databases,logical_capacity_bytes,host,direct_port,pooled_port) VALUES($1,$2,$3,$4,$5::uuid,$6,$7,$8,$9,$10,$11,$12) RETURNING {returning} AS value"),vec![id.clone().into(),cluster.into(),kind.into(),text(pool,"node")?.into(),text(pool,"poolID")?.into(),demand.into(),profile.into(),(if shared{8_i32}else{1}).into(),(if shared{8589934592_i64}else{0}).into(),format!("db-{id}.{suffix}").into(),(ports[0] as i32).into(),(ports[1] as i32).into()]).await?;
        let mut insert_ports = vec![];
        for port in ports {
            insert_ports.push(one(tx,"INSERT INTO webdock_auth.hosting_database_port(cluster_id,port,backend_id) VALUES($1,$2,$3) RETURNING jsonb_build_object('id',port::text,'tableOid',tableoid::oid::text,'xmin',xmin::text) AS value",vec![cluster.into(),(port as i32).into(),id.clone().into()]).await?);
        }
        let backend = {
            let mut backend = backend;
            backend["_insertPorts"] = json!(insert_ports);
            backend
        };
        Ok(backend)
    }
}

impl DatabaseService {
    /// Operator-only, public capacity metadata for registering qualified pools and shared installations.
    pub async fn capacity_options(
        &self,
        principal: &Principal,
        authority: &StoredAuthority,
    ) -> Result<Value> {
        let (auth, current) = self
            .verify(&principal.user_id, &principal.session_id, authority)
            .await?;
        let tx = self.begin_actor(&current).await?;
        self.installed(&tx, false).await?;
        self.authorize_mode(&tx, &current, None, None, false, true, authority, false)
            .await?;
        let rows=all(&tx,"SELECT jsonb_build_object('id',id,'name',name,'snapshot',webdock_rls.cluster_snapshot(id)) AS value FROM webdock_auth.hosting_cluster WHERE ownership='platform' ORDER BY name,id LIMIT 100",vec![]).await?;
        let mut output = vec![];
        for row in rows {
            let c = &row["snapshot"];
            let seen = c["last_seen"]
                .as_str()
                .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok());
            let ready = c["verified"] == true
                && c["revoked"] != true
                && c["observation"]["capabilities"]["databaseVersion"] == 1
                && seen.is_some_and(|t| {
                    chrono::Utc::now()
                        .signed_duration_since(t)
                        .num_milliseconds()
                        < 90000
                });
            let registered=all(&tx,"SELECT jsonb_build_object('node',node,'poolID',pool_id,'capacityBytes',capacity_bytes) AS value FROM webdock_auth.hosting_database_pool WHERE cluster_id=$1",vec![text(&row,"id")?.into()]).await?;
            let pools=c["observation"]["capabilities"]["databasePools"].as_array().map(|ps|ps.iter().map(|p|json!({"node":p["node"],"poolID":p["poolID"],"capacityBytes":p["capacityBytes"],"availableBytes":p["availableBytes"],"ready":ready&&p["ready"]==true,"registered":registered.iter().any(|r|r["node"]==p["node"]&&r["poolID"]==p["poolID"]&&r["capacityBytes"]==p["capacityBytes"])})).collect::<Vec<_>>()).unwrap_or_default();
            output.push(json!({"id":row["id"],"name":row["name"],"revision":c["revision"].as_i64().unwrap_or(0),"ready":ready,"pools":pools}));
        }
        tx.commit().await?;
        auth.commit().await?;
        Ok(json!(output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn placement_accounts_for_unknown_allocations_and_pending_physical_reservations() {
        let mut pool = json!({"requestedCpuMillicores":0,"requestedMemoryBytes":0,"capacityBytes":1000,"availableBytes":900,"allocatedBytes":100});
        let held = json!({"held_cpu":100,"held_memory":100,"held_volume":200,"known_cpu":0,"known_memory":0,"allocated":100});
        let node = json!({"cpuMillicores":1000,"memoryBytes":1000});
        let apps = json!({"cpuMillicores":100,"memoryBytes":100});
        let demand = json!({"cpuMillicores":200,"memoryBytes":200,"volumeBytes":300});
        assert!(fits(&pool, &held, &node, &apps, &demand).unwrap());
        pool["allocatedBytes"] = json!(800);
        assert!(!fits(&pool, &held, &node, &apps, &demand).unwrap());
        pool["allocatedBytes"] = json!(100);
        pool["availableBytes"] = json!(399);
        assert!(!fits(&pool, &held, &node, &apps, &demand).unwrap());
        pool["availableBytes"] = json!(900);
        pool["requestedCpuMillicores"] = json!(401);
        assert!(!fits(&pool, &held, &node, &apps, &demand).unwrap());
    }
}
