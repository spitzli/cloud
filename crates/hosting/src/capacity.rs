use super::*;
impl HostingService {
    pub(super) async fn limits(
        tx: &DatabaseTransaction,
        customer: &str,
        project: Option<&str>,
        provider: Option<&str>,
    ) -> Result<Value> {
        let sub=rows(tx,"SELECT to_jsonb(s) AS value FROM webdock_auth.tenant_subscription s WHERE customer_id=$1",vec![customer.into()]).await?.pop().unwrap_or(json!({}));
        let base = contracts::allowances(sub.get("base").and_then(|v| v.get("hosting")))?;
        let extra = contracts::allowances(sub.get("extras").and_then(|v| v.get("hosting")))?;
        let mut effective = json!({});
        for key in DIMENSIONS {
            if extra[key].is_null() {
                return Err(error(503, "Hosting extras must be finite."));
            }
            effective[key] = if base[key].is_null() {
                Value::Null
            } else {
                let n = amount(&base[key])?
                    .checked_add(amount(&extra[key])?)
                    .filter(|n| *n <= contracts::MAX_SAFE)
                    .ok_or_else(|| error(503, "Hosting allowance exceeds supported range."))?;
                json!(n)
            };
        }
        let limits=rows(tx,"SELECT jsonb_build_object('scope',scope,'values',values,'revision',revision) AS value FROM webdock_auth.hosting_limit WHERE customer_id=$1 ORDER BY scope",vec![customer.into()]).await?;
        for limit in &limits {
            let scope = field(limit, "scope")?;
            if scope != "customer"
                && !project.is_some_and(|id| scope == format!("project:{id}"))
                && !provider.is_some_and(|id| scope == format!("provider:{id}"))
            {
                continue;
            }
            for key in DIMENSIONS {
                let Some(value) = limit["values"].get(key).filter(|v| !v.is_null()) else {
                    continue;
                };
                let ceiling = amount(value)?;
                effective[key] = json!(if effective[key].is_null() {
                    ceiling
                } else {
                    ceiling.min(amount(&effective[key])?)
                });
            }
        }
        Ok(
            json!({"effective":effective,"subscriptionRevision":sub.get("revision").cloned().unwrap_or(json!(0)),"limits":limits}),
        )
    }
    pub(super) async fn cluster_view(tx: &DatabaseTransaction, id: &str) -> Result<Value> {
        let mut value: Value = one(
            tx,
            "SELECT webdock_rls.cluster_snapshot($1) AS value",
            vec![id.into()],
        )
        .await?
        .ok_or_else(|| error(404, "Cluster is unavailable."))?
        .try_get::<Option<Value>>("", "value")
        .map_err(db_error)?
        .ok_or_else(|| error(404, "Cluster is unavailable."))?;
        let kind: String = one(
            tx,
            "SELECT webdock_rls.hosting_runtime_kind($1) AS kind",
            vec![id.into()],
        )
        .await?
        .ok_or_else(|| error(404, "Cluster unavailable."))?
        .try_get("", "kind")
        .map_err(db_error)?;
        let fresh = epoch(&value["lastSeen"]).is_some_and(|time| now() - time < 90);
        value["state"] = json!(if value["revoked"] == true {
            "revoked"
        } else if value["lastSeen"].is_null() {
            "unconnected"
        } else if fresh {
            "connected"
        } else {
            "stale"
        });
        value["workloadReady"] =
            json!(value["verified"] == true && fresh && value["revoked"] != true);
        value["lastSeen"] = timestamp(&value["lastSeen"]);
        value["runtimeKind"] = json!(kind);
        value.as_object_mut().unwrap().remove("last_seen");
        Ok(value)
    }
    pub(super) async fn check_demand(
        tx: &DatabaseTransaction,
        customer: &str,
        project: &str,
        cluster: &str,
        provider: &str,
        demand: &Value,
        subscription: i64,
    ) -> Result<()> {
        let caps = Self::limits(tx, customer, Some(project), Some(provider)).await?;
        if numeric(&caps, "subscriptionRevision")? != subscription {
            return Err(error(
                409,
                "The subscription changed. Refresh and try again.",
            ));
        }
        let snapshot = Self::cluster_view(tx, cluster).await?;
        if snapshot["workloadReady"] != true {
            return Err(error(
                409,
                "Cluster readiness must be verified before allocation.",
            ));
        }
        let cluster_caps = contracts::allowances(
            snapshot
                .get("capacity")
                .filter(|v| !v.is_null())
                .or_else(|| snapshot["observation"].get("capacity")),
        )?;
        let cluster_used: Value = one(
            tx,
            "SELECT webdock_rls.capacity_demand($1,'all',NULL) AS demand",
            vec![cluster.into()],
        )
        .await?
        .ok_or_else(|| error(503, "Cluster capacity unavailable."))?
        .try_get("", "demand")
        .map_err(db_error)?;
        let allocations=rows(tx,"SELECT to_jsonb(r)||jsonb_build_object('provider',h.provider) AS value FROM webdock_auth.hosting_reservation r JOIN webdock_auth.hosting_project h ON h.project_id=r.project_id WHERE r.status<>'released' AND r.customer_id=$1",vec![customer.into()]).await?;
        let total = contracts::totals(&allocations)?;
        let global = Self::limits(tx, customer, None, None).await?;
        for key in DIMENSIONS {
            let requested = amount(&demand[key])?;
            if requested == 0 {
                continue;
            }
            let sum = |used: u64| -> Result<u64> {
                if key == "replicasPerApp" {
                    Ok(requested)
                } else {
                    used.checked_add(requested)
                        .filter(|n| *n <= contracts::MAX_SAFE)
                        .ok_or_else(|| error(409, "Allocated resources exceed supported range."))
                }
            };
            if !global["effective"][key].is_null()
                && sum(amount(&total[key])?)? > amount(&global["effective"][key])?
            {
                return Err(error(409, "Customer hosting allowance exceeded."));
            }
            for limit in caps["limits"].as_array().unwrap() {
                let scope = field(limit, "scope")?;
                if scope != "customer"
                    && scope != format!("provider:{provider}")
                    && scope != format!("project:{project}")
                {
                    continue;
                }
                if let Some(ceiling) = limit["values"].get(key).filter(|v| !v.is_null()) {
                    let relevant = allocations
                        .iter()
                        .filter(|row| {
                            scope == "customer"
                                || scope
                                    == format!(
                                        "provider:{}",
                                        row["provider"].as_str().unwrap_or("")
                                    )
                                || scope
                                    == format!(
                                        "project:{}",
                                        row["project_id"].as_str().unwrap_or("")
                                    )
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    if sum(amount(&contracts::totals(&relevant)?[key])?)? > amount(ceiling)? {
                        return Err(error(
                            409,
                            "Project or provider hosting allowance exceeded.",
                        ));
                    }
                }
            }
            if cluster_caps[key].is_null()
                || sum(amount(&cluster_used[key])?)? > amount(&cluster_caps[key])?
            {
                return Err(error(409, "Verified cluster capacity is insufficient."));
            }
        }
        Ok(())
    }
    pub(super) async fn usage(tx: &DatabaseTransaction, customer: &str) -> Result<Value> {
        let mut limits = Self::limits(tx, customer, None, None).await?;
        let allocations=rows(tx,"SELECT jsonb_build_object('demand',demand,'status',status) AS value FROM webdock_auth.hosting_reservation WHERE customer_id=$1 AND status<>'released'",vec![customer.into()]).await?;
        let total = contracts::totals(&allocations)?;
        limits["reserved"] = contracts::totals(
            &allocations
                .iter()
                .filter(|r| r["status"] == "reserved")
                .cloned()
                .collect::<Vec<_>>(),
        )?;
        limits["allocated"] = contracts::totals(
            &allocations
                .iter()
                .filter(|r| r["status"] == "active")
                .cloned()
                .collect::<Vec<_>>(),
        )?;
        limits["measured"] = Value::Null;
        limits["checkedAt"] = Value::Null;
        limits["enforcement"] = json!("webdock-enforced");
        limits["overallocated"] =
            json!(DIMENSIONS.iter().any(|k| !limits["effective"][k].is_null()
                && total[k].as_u64().unwrap_or(0) > limits["effective"][k].as_u64().unwrap_or(0)));
        Ok(limits)
    }
}

#[cfg(test)]
mod stored_provider_tests {
    use super::*;
    #[test]
    fn per_provider_allocations_use_stored_values_without_cluster_visibility() {
        let source = include_str!("capacity.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(source.contains("jsonb_build_object('provider',h.provider) AS value FROM webdock_auth.hosting_reservation"));
        assert!(!source.contains("LEFT JOIN webdock_auth.hosting_cluster"));
        assert!(!source.contains("native_provider::"));
        assert!(source.contains("scope != format!(\"provider:{provider}\")"));
    }
    #[test]
    fn true_backend_scopes_count_active_reserved_and_uncertain_without_cross_provider_reassignment()
    {
        let mut allocations = Vec::new();
        for provider in ["k3s", "docker", "vercel"] {
            for (status, operation, bytes) in [
                ("active", "succeeded", 10),
                ("reserved", "queued", 20),
                ("reserved", "needs-reconciliation", 30),
            ] {
                allocations.push(json!({"provider":provider,"status":status,"operationStatus":operation,"demand":{"apps":1,"cpuMillicores":100,"memoryBytes":134217728,"ephemeralBytes":1048576,"volumeBytes":bytes,"replicasPerApp":1,"concurrentDeployments":1}}));
            }
        }
        for provider in ["k3s", "docker", "vercel"] {
            let matching = allocations
                .iter()
                .filter(|r| r["provider"] == provider)
                .cloned()
                .collect::<Vec<_>>();
            let total = contracts::totals(&matching).unwrap();
            assert_eq!(total["volumeBytes"], 60);
            assert_eq!(total["apps"], 3);
            assert_eq!(total["concurrentDeployments"], 2);
            assert_eq!(total["cpuMillicores"], 300);
        }
        assert_eq!(contracts::totals(&allocations).unwrap()["volumeBytes"], 180);
    }
}
