//! Core admission owns recovery identities and the complete retained peak. No customer/project is invented.
use super::*;
const SCRATCH: u64 = 67108864;
fn check_pool_capacity(
    cluster_volume: u64,
    pool_recovery: u64,
    additional: u64,
    capacity: u64,
) -> Result<()> {
    // Legacy allocation records lack a pool identity. Charge all canonical cluster
    // volume demand conservatively; it already includes retained recovery peaks.
    if checked_sum([cluster_volume.max(pool_recovery), additional])? > capacity {
        return Err(error(
            409,
            "Recovery pool capacity cannot reserve the complete retained peak.",
        ));
    }
    Ok(())
}
fn checked_sum(values: impl IntoIterator<Item = u64>) -> Result<u64> {
    values.into_iter().try_fold(0u64, |n, v| {
        n.checked_add(v)
            .filter(|n| *n <= contracts::MAX_SAFE)
            .ok_or_else(failure)
    })
}

fn volume_plan(target: &Value, set: &Value) -> Result<(Value, Value, u64)> {
    let plan = target["volumePlan"].as_object().ok_or_else(failure)?;
    if plan.len() > 102 {
        return Err(failure());
    }
    let mut demand = contracts::allowances(None)?;
    let mut customers = std::collections::BTreeSet::new();
    let mut core = 0;
    let mut private = 0;
    let mut bytes = 0;
    for (name, volume) in plan {
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            return Err(failure());
        }
        let size = contracts::amount(&volume["bytes"])?;
        if !(67108864..=68719476736).contains(&size)
            || volume["uid"]
                .as_u64()
                .is_none_or(|uid| uid == 0 || uid > 65535)
            || volume["gid"]
                .as_u64()
                .is_none_or(|gid| gid == 0 || gid > 65535)
        {
            return Err(failure());
        }
        let image = volume["image"].as_str().ok_or_else(failure)?;
        if !webdock_supervisor::image_digest(image) {
            return Err(failure());
        }
        match volume["role"]["role"].as_str() {
            Some("core-ledger-postgres") => core += 1,
            Some("private-mail-postgres") => private += 1,
            Some("incoming-ce") => {
                let customer = volume["role"]["originalCustomerID"]
                    .as_str()
                    .ok_or_else(failure)?;
                contracts::record_id(customer)?;
                if !customers.insert(customer.to_owned())
                    || volume["uid"] != 65532
                    || volume["gid"] != 65532
                {
                    return Err(failure());
                }
                let source = set["proof"]["ce"]
                    .as_array()
                    .and_then(|ce| ce.iter().find(|source| source["customerID"] == customer))
                    .ok_or_else(failure)?;
                if source["image"] != image
                    || source["sourceVolumeBytes"]
                        .as_u64()
                        .is_some_and(|required| required > size)
                {
                    return Err(failure());
                }
            }
            _ => return Err(failure()),
        }
        let cpu = contracts::amount(&volume["cpuMillicores"])?;
        let memory = contracts::amount(&volume["memoryBytes"])?;
        let temporary = contracts::amount(&volume["tmpBytes"])?;
        if !(100..=8000).contains(&cpu)
            || !(134217728..=8589934592).contains(&memory)
            || !(1048576..=1073741824).contains(&temporary)
        {
            return Err(failure());
        }
        demand["apps"] = json!(contracts::amount(&demand["apps"])? + 1);
        demand["cpuMillicores"] = json!(checked_sum([
            contracts::amount(&demand["cpuMillicores"])?,
            cpu
        ])?);
        demand["memoryBytes"] = json!(checked_sum([
            contracts::amount(&demand["memoryBytes"])?,
            memory
        ])?);
        demand["ephemeralBytes"] = json!(checked_sum([
            contracts::amount(&demand["ephemeralBytes"])?,
            temporary
        ])?);
        bytes = checked_sum([bytes, size])?;
    }
    let expected = set["proof"]["ce"]
        .as_array()
        .ok_or_else(failure)?
        .iter()
        .map(|source| {
            source["customerID"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(failure)
        })
        .collect::<Result<std::collections::BTreeSet<_>>>()?;
    if core != 1 || private != 1 || customers != expected {
        return Err(failure());
    }
    // Read-only TLS gateways are real bounded runtimes too.
    demand["cpuMillicores"] = json!(checked_sum([
        contracts::amount(&demand["cpuMillicores"])?,
        customers.len() as u64 * 100
    ])?);
    demand["memoryBytes"] = json!(checked_sum([
        contracts::amount(&demand["memoryBytes"])?,
        customers.len() as u64 * 134217728
    ])?);
    demand["ephemeralBytes"] = json!(checked_sum([
        contracts::amount(&demand["ephemeralBytes"])?,
        customers.len() as u64 * 16777216
    ])?);
    Ok((Value::Object(plan.clone()), demand, bytes))
}
impl HostingService {
    pub(super) async fn reserve_recovery_target(
        &self,
        tx: &DatabaseTransaction,
        p: &Value,
        spec: &RecoverySpec,
        operation: &str,
    ) -> Result<()> {
        if spec.action != RecoveryAction::Restore {
            return self.reserve_recovery_backup(tx, p, spec, operation).await;
        }
        let target = spec.target.as_ref().ok_or_else(failure)?;
        let set=one(tx,"SELECT to_jsonb(s) AS value FROM webdock_auth.hosting_recovery_set s WHERE id=$1 AND policy_id=$2 AND referenced",vec![spec.recovery_set_id.clone().ok_or_else(failure)?.into(),spec.policy_id.clone().into()]).await?.map(object).transpose()?.ok_or_else(failure)?;
        let object_receipt = set["proof"]["objects"]
            .as_array()
            .and_then(|objects| objects.first())
            .ok_or_else(failure)?;
        let encrypted = contracts::amount(&object_receipt["sizeBytes"])?;
        if encrypted == 0 || encrypted > 68719476736 {
            return Err(failure());
        }
        let staged = checked_sum([encrypted, encrypted, spec.capacity_bytes, SCRATCH])?;
        let (volumes, mut demand, target_bytes) =
            if spec.profile == RecoveryProfile::MailInstallationV1 {
                volume_plan(target, &set)?
            } else {
                let demand = target["reservationDemand"].clone();
                let runtime: &Value = &target["runtime"];
                (
                    json!({}),
                    demand,
                    contracts::amount(&runtime["volume"]["sizeBytes"])?,
                )
            };
        let peak = json!({"encryptedBytes":encrypted,"plaintextBytes":encrypted,"extractedBytes":spec.capacity_bytes,"materialAndScratchBytes":SCRATCH,"targetBytes":target_bytes});
        let peak_bytes = checked_sum([staged, target_bytes])?;
        let pool = &target["storageIdentity"];
        if pool["configuration"]["clusterID"] != spec.cluster_id
            || pool["poolUID"].as_str().is_none()
            || pool["revision"].as_str().is_none_or(|s| s.len() != 64)
        {
            return Err(failure());
        }
        let pool_id = field(&pool["configuration"], "poolID")?;
        let mut customers = std::collections::BTreeSet::new();
        for volume in volumes.as_object().ok_or_else(failure)?.values() {
            if let Some(id) = volume["role"]["originalCustomerID"].as_str() {
                customers.insert(id.to_owned());
            }
        }
        for customer in customers {
            Self::lock(tx, format!("plan:{customer}")).await?;
        }
        Self::lock(tx, format!("hosting-cluster:{}", spec.cluster_id)).await?;
        let current=one(tx,"SELECT observation#>'{capabilities,nativeRecovery}' AS value FROM webdock_auth.hosting_agent WHERE cluster_id=$1 AND NOT revoked AND last_seen>clock_timestamp()-interval '90 seconds' FOR SHARE",vec![spec.cluster_id.clone().into()]).await?.map(object).transpose()?.ok_or_else(failure)?;
        if current["version"] != 2
            || !current["targets"].as_array().is_some_and(|targets| {
                targets.iter().any(|declared| {
                    declared["id"] == target["profileID"]
                        && declared["sha256"] == target["sha256"]
                        && declared["nonce"] == target["nonce"]
                        && declared["storageIdentity"] == *pool
                })
            })
        {
            return Err(failure());
        }
        let used=one(tx,"SELECT coalesce(sum(peak_bytes),0)::bigint AS bytes FROM webdock_auth.hosting_recovery_reservation WHERE cluster_id=$1 AND pool_id=$2::uuid AND state<>'released'",vec![spec.cluster_id.clone().into(),pool_id.into()]).await?.ok_or_else(failure)?.try_get::<i64>("","bytes").map_err(db_error)?;
        let pool_capacity = contracts::amount(&pool["configuration"]["capacityBytes"])?;
        // The generic cluster budget includes all current Hosting/PG/Mail allocations.
        let existing = one(
            tx,
            "SELECT webdock_rls.capacity_demand($1,'all',NULL) AS value",
            vec![spec.cluster_id.clone().into()],
        )
        .await?
        .map(object)
        .transpose()?
        .ok_or_else(failure)?;
        check_pool_capacity(
            contracts::amount(&existing["volumeBytes"])?,
            used as u64,
            peak_bytes,
            pool_capacity,
        )?;
        let cluster = Self::cluster_view(tx, &spec.cluster_id).await?;
        let limits = contracts::allowances(
            cluster
                .get("capacity")
                .filter(|v| !v.is_null())
                .or_else(|| cluster["observation"].get("capacity")),
        )?;
        if spec.profile == RecoveryProfile::MailInstallationV1 {
            demand["volumeBytes"] = json!(peak_bytes);
            demand["cpuMillicores"] = json!(checked_sum([
                contracts::amount(&demand["cpuMillicores"])?,
                100
            ])?);
            demand["memoryBytes"] = json!(checked_sum([
                contracts::amount(&demand["memoryBytes"])?,
                268435456
            ])?);
            demand["ephemeralBytes"] = json!(checked_sum([
                contracts::amount(&demand["ephemeralBytes"])?,
                SCRATCH
            ])?);
            demand["concurrentDeployments"] = json!(1);
            for dimension in DIMENSIONS {
                if !limits[dimension].is_null()
                    && checked_sum([
                        contracts::amount(&existing[dimension])?,
                        contracts::amount(&demand[dimension])?,
                    ])? > contracts::amount(&limits[dimension])?
                {
                    return Err(error(
                        409,
                        "Cluster capacity cannot reserve recovery runtimes and staging.",
                    ));
                }
            }
        }
        let reservation = self.id().await?;
        execute(tx,"INSERT INTO webdock_auth.hosting_recovery_reservation(id,operation_id,policy_id,policy_revision,cluster_id,pool_id,pool_identity,target_nonce,target_sha256,source_set_id,peak,peak_bytes,demand,charged_via_hosting,state) VALUES($1,$2,$3,$4,$5,$6::uuid,$7,$8::uuid,$9,$10,$11,$12,$13,$14,'reserved')",vec![reservation.clone().into(),operation.into(),spec.policy_id.clone().into(),(spec.policy_revision as i32).into(),spec.cluster_id.clone().into(),pool_id.into(),pool.clone().into(),field(target,"nonce")?.into(),field(target,"sha256")?.into(),spec.recovery_set_id.clone().into(),peak.into(),(peak_bytes as i64).into(),demand.into(),(spec.profile==RecoveryProfile::NtfySqliteOfflineV1).into()]).await?;
        for (name, volume) in volumes.as_object().ok_or_else(failure)? {
            let resource = self.id().await?;
            execute(tx,"INSERT INTO webdock_auth.hosting_recovery_target_volume(resource_id,reservation_id,role_name,role,original_customer_id,volume_bytes,image,uid,gid,cpu_millicores,memory_bytes,tmp_bytes) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",vec![resource.into(),reservation.clone().into(),name.clone().into(),volume["role"].clone().into(),volume["role"]["originalCustomerID"].as_str().into(),contracts::amount(&volume["bytes"])?.into(),field(volume,"image")?.into(),(contracts::amount(&volume["uid"])? as i64).into(),(contracts::amount(&volume["gid"])? as i64).into(),(contracts::amount(&volume["cpuMillicores"])? as i64).into(),(contracts::amount(&volume["memoryBytes"])? as i64).into(),(contracts::amount(&volume["tmpBytes"])? as i64).into()]).await?;
        }
        let _ = p;
        Ok(())
    }
    async fn reserve_recovery_backup(
        &self,
        tx: &DatabaseTransaction,
        p: &Value,
        spec: &RecoverySpec,
        operation: &str,
    ) -> Result<()> {
        let pool = &p["destination"]["storageIdentity"];
        if pool["configuration"]["clusterID"] != spec.cluster_id {
            return Err(failure());
        }
        if let Some(customer) = spec.source["customerID"].as_str() {
            Self::lock(tx, format!("plan:{customer}")).await?;
        }
        Self::lock(tx, format!("hosting-cluster:{}", spec.cluster_id)).await?;
        let scratch = SCRATCH
            .checked_add(if spec.profile == RecoveryProfile::MailInstallationV1 {
                spec.capacity_bytes
            } else {
                0
            })
            .ok_or_else(failure)?;
        let peak = json!({"encryptedBytes":spec.capacity_bytes,"plaintextBytes":spec.capacity_bytes,"extractedBytes":spec.capacity_bytes,"materialAndScratchBytes":scratch,"targetBytes":0});
        let bytes = checked_sum([
            spec.capacity_bytes,
            spec.capacity_bytes,
            spec.capacity_bytes,
            scratch,
        ])?;
        let demand = json!({"apps":0,"cpuMillicores":100,"memoryBytes":268435456,"volumeBytes":bytes,"ephemeralBytes":SCRATCH,"replicasPerApp":0,"concurrentDeployments":1});
        let current = one(
            tx,
            "SELECT webdock_rls.capacity_demand($1,'all',NULL) AS value",
            vec![spec.cluster_id.clone().into()],
        )
        .await?
        .map(object)
        .transpose()?
        .ok_or_else(failure)?;
        let cluster = Self::cluster_view(tx, &spec.cluster_id).await?;
        let caps = contracts::allowances(
            cluster
                .get("capacity")
                .filter(|v| !v.is_null())
                .or_else(|| cluster["observation"].get("capacity")),
        )?;
        for dimension in DIMENSIONS {
            if !caps[dimension].is_null()
                && checked_sum([
                    contracts::amount(&current[dimension])?,
                    contracts::amount(&demand[dimension])?,
                ])? > contracts::amount(&caps[dimension])?
            {
                return Err(error(
                    409,
                    "Capacity cannot reserve every simultaneous backup copy.",
                ));
            }
        }
        let used=one(tx,"SELECT coalesce(sum(peak_bytes),0)::bigint AS bytes FROM webdock_auth.hosting_recovery_reservation WHERE cluster_id=$1 AND pool_id=$2::uuid AND state<>'released'",vec![spec.cluster_id.clone().into(),field(&pool["configuration"],"poolID")?.into()]).await?.ok_or_else(failure)?.try_get::<i64>("","bytes").map_err(db_error)?;
        check_pool_capacity(
            contracts::amount(&current["volumeBytes"])?,
            used as u64,
            bytes,
            contracts::amount(&pool["configuration"]["capacityBytes"])?,
        )?;
        let id = self.id().await?;
        execute(tx,"INSERT INTO webdock_auth.hosting_recovery_reservation(id,operation_id,policy_id,policy_revision,cluster_id,pool_id,pool_identity,target_nonce,target_sha256,source_set_id,peak,peak_bytes,demand,charged_via_hosting,state) VALUES($1,$2,$3,$4,$5,$6::uuid,$7,$8::uuid,$9,NULL,$10,$11,$12,false,'reserved')",vec![id.into(),operation.into(),spec.policy_id.clone().into(),(spec.policy_revision as i32).into(),spec.cluster_id.clone().into(),field(&pool["configuration"],"poolID")?.into(),pool.clone().into(),uuid_token().into(),field(&p["destination"],"sha256")?.into(),peak.into(),(bytes as i64).into(),demand.into()]).await?;
        Ok(())
    }
    pub(super) async fn recovery_reservation_plan(
        &self,
        tx: &DatabaseTransaction,
        agent: &AgentIdentity,
        c: &RecoveryClaim,
    ) -> Result<Value> {
        let row=one(tx,"SELECT to_jsonb(r) AS value FROM webdock_auth.hosting_recovery_reservation r WHERE operation_id=$1 AND policy_id=$2 AND policy_revision=$3 AND state IN('reserved','held') FOR SHARE",vec![c.spec.operation_id.clone().into(),c.spec.policy_id.clone().into(),(c.spec.policy_revision as i32).into()]).await?.map(object).transpose()?.ok_or_else(failure)?;
        let identity=one(tx,"SELECT agent_identity AS value FROM webdock_auth.hosting_recovery_job WHERE operation_id=$1",vec![c.spec.operation_id.clone().into()]).await?.map(object).transpose()?.ok_or_else(failure)?;
        let installation = identity["identity"]
            .as_str()
            .and_then(|s| s.strip_prefix("agent:"))
            .and_then(|s| s.split(':').next())
            .filter(|s| !s.is_empty())
            .ok_or_else(failure)?;
        let volumes=rows(tx,"SELECT jsonb_build_object('name',role_name,'value',jsonb_build_object('resourceID',resource_id,'volumeBytes',volume_bytes,'role',role,'uid',uid,'gid',gid,'image',image)) AS value FROM webdock_auth.hosting_recovery_target_volume WHERE reservation_id=$1 ORDER BY role_name",vec![field(&row,"id")?.into()]).await?;
        let mut planned = serde_json::Map::new();
        for volume in volumes {
            planned.insert(field(&volume, "name")?.into(), volume["value"].clone());
        }
        let backup_target;
        let target = if let Some(target) = c.spec.target.as_ref() {
            target
        } else {
            backup_target = json!({"profileID":c.spec.source["profileID"],"sha256":row["target_sha256"],"nonce":row["target_nonce"]});
            &backup_target
        };
        let application = if c.spec.profile == RecoveryProfile::NtfySqliteOfflineV1
            && c.spec.action == RecoveryAction::Restore
        {
            let runtime: webdock_supervisor::RuntimeSpec =
                serde_json::from_value(target["runtime"].clone()).map_err(|_| failure())?;
            json!({"appID":runtime.resource_id,"projectID":runtime.project_id,"clusterID":agent.cluster,"revision":runtime.revision,"volumeBytes":runtime.volume.and_then(|v|v.size_bytes).ok_or_else(failure)?})
        } else {
            Value::Null
        };
        let application_source = if c.spec.profile == RecoveryProfile::NtfySqliteOfflineV1
            && c.spec.action == RecoveryAction::Backup
        {
            json!({"appID":c.spec.source["appID"],"projectID":c.spec.source["projectID"],"clusterID":c.spec.cluster_id,"revision":c.spec.source["revision"],"volumeBytes":c.spec.source["spec"]["volumeBytes"]})
        } else {
            Value::Null
        };
        let grant = json!({"version":1,"scope":match(c.spec.profile,c.spec.action){(RecoveryProfile::MailInstallationV1,RecoveryAction::Restore)=>"installation-mail-recovery",(RecoveryProfile::MailInstallationV1,RecoveryAction::Backup)=>"installation-mail-backup",(_,RecoveryAction::Restore)=>"application-recovery",(_,RecoveryAction::Backup)=>"application-backup"},"profile":c.spec.profile,"action":c.spec.action,"installationID":installation,"operationID":c.spec.operation_id,"generation":c.lease.generation,"policyID":c.spec.policy_id,"policyRevision":c.spec.policy_revision,"agentGeneration":agent.generation,"leaseHash":digest(&c.lease.token),"epoch":c.spec.epoch,"reservationID":row["id"],"sourceSetID":c.spec.recovery_set_id,"targetProfileId":target["profileID"],"targetProfileSha256":target["sha256"],"targetNonce":target["nonce"],"pool":row["pool_identity"],"peak":row["peak"],"volumes":planned,"applicationTarget":application,"applicationSource":application_source,"expiresAt":c.lease.expires_at});
        let signed = self
            .recovery_signer
            .as_ref()
            .ok_or_else(|| error(503, "Independent recovery signer is not configured."))?
            .sign_reservation(&grant)?;
        Ok(
            json!({"authority":{"origin":"core","document":signed},"targetProfileId":target["profileID"],"targetProfileSha256":target["sha256"],"targetNonce":target["nonce"],"pool":row["pool_identity"]}),
        )
    }
}

#[cfg(test)]
mod pool_capacity_tests {
    use super::*;
    #[test]
    fn retained_and_uncertain_legacy_allocations_cannot_be_spent_on_recovery() {
        // Canonical cluster volume demand already contains every held recovery
        // peak and the ordinary retained/uncertain allocation ledgers.
        assert!(check_pool_capacity(900, 200, 200, 1000).is_err());
        assert!(check_pool_capacity(900, 0, 200, 1000).is_err());
    }
    #[test]
    fn canonical_recovery_peaks_are_not_added_twice() {
        check_pool_capacity(900, 200, 100, 1000).unwrap();
        assert!(check_pool_capacity(0, 900, 200, 1000).is_err());
        assert!(check_pool_capacity(u64::MAX, 0, 1, u64::MAX).is_err());
    }
}
