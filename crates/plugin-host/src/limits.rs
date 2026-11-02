//! Plans and limits. Core refuses what would take a customer past a limit of their plan: resource
//! creates at admission, workload allocations when it pins them, projects in the database.
use super::*;

/// How a meter reads in a message: (`memory`, 1536) → "1536 MiB".
fn amount(meter: &str, value: u64) -> String {
    match meter {
        "memory" | "storage" => format!("{value} MiB"),
        "cpu" => format!("{} CPU", value as f64 / 1000.0),
        _ => value.to_string(),
    }
}
fn noun(meter: &str) -> &str {
    match meter {
        "memory" | "storage" | "projects" => meter,
        "cpu" => "",
        _ => "of this kind",
    }
}
/// The first meter `wanted` grows past its limit, as a message for the customer. Shrinking is
/// always allowed, even when the customer is already over (after a plan change).
pub(crate) fn exceeded(checked: &Value, previous: &Value, wanted: &Value) -> Option<String> {
    let limits = &checked["limits"];
    let allocated = &checked["allocated"];
    for (meter, value) in wanted.as_object()? {
        let wanted = value.as_u64().unwrap_or(0);
        let previous = previous[meter].as_u64().unwrap_or(0);
        let Some(limit) = limits[meter].as_u64() else {
            continue;
        };
        let others = allocated[meter]
            .as_u64()
            .unwrap_or(0)
            .saturating_sub(previous);
        if wanted > previous && others.saturating_add(wanted) > limit {
            if meter.contains('.') {
                return Some(format!(
                    "Your plan allows {limit} of these and {others} are in use."
                ));
            }
            let limit = format!("{} {}", amount(meter, limit), noun(meter));
            return Some(format!(
                "Your plan allows {}; {} are in use and this needs {}.",
                limit.trim_end(),
                amount(meter, others),
                amount(meter, wanted)
            ));
        }
    }
    None
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn growth_past_a_limit_is_refused_and_shrinking_is_not() {
        let checked = json!({"limits":{"memory":1024,"hosting.app":2},"allocated":{"memory":768,"cpu":400,"hosting.app":2}});
        // A new unit of 512 MiB: 768 + 512 > 1024.
        assert_eq!(
            exceeded(&checked, &json!({}), &json!({"memory":512,"cpu":50})).as_deref(),
            Some("Your plan allows 1024 MiB memory; 768 MiB are in use and this needs 512 MiB.")
        );
        // Resizing a unit of 256 MiB to 512 MiB: 512 others + 512 ≤ 1024.
        assert_eq!(
            exceeded(&checked, &json!({"memory":256}), &json!({"memory":512})),
            None
        );
        // Over after a plan change, but shrinking or staying the same passes.
        let over = json!({"limits":{"memory":256},"allocated":{"memory":1024}});
        assert_eq!(
            exceeded(&over, &json!({"memory":512}), &json!({"memory":256})),
            None
        );
        assert_eq!(
            exceeded(&over, &json!({"memory":512}), &json!({"memory":512})),
            None
        );
        // Unlimited meters never refuse.
        assert_eq!(exceeded(&checked, &json!({}), &json!({"cpu":100000})), None);
        // Counts: a third app.
        assert_eq!(
            exceeded(&checked, &json!({}), &json!({"hosting.app":1})).as_deref(),
            Some("Your plan allows 2 of these and 2 are in use.")
        );
    }
}

type Authority<'a> = Option<&'a webdock_browser_session::StoredAuthority>;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanInput {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub limits: Value,
    #[serde(default)]
    pub is_default: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomerPlanInput {
    pub plan_id: Option<String>,
    #[serde(default = "empty")]
    pub overrides: Value,
}
fn empty() -> Value {
    json!({})
}
fn record(id: &str) -> bool {
    (1..=19).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit()) && !id.starts_with('0')
}
/// Database refusals of a plan write, in the operator's words.
fn plan_error(error: sea_orm::DbErr) -> PluginError {
    let text = error.to_string();
    if text.contains("plan_limits_check") || text.contains("plan_overrides_check") {
        invalid("Limits are whole numbers of zero or more")
    } else if text.contains("plan_default") {
        conflict("Another plan is already the default")
    } else if text.contains("customer_plan_plan_id_fkey") {
        conflict("Customers are still on this plan")
    } else if text.contains("plan_name_check") || text.contains("plan_description_check") {
        invalid("Enter a name of up to 80 characters and a description of up to 500")
    } else {
        unavailable()
    }
}
impl HostService {
    /// Everything a plan can limit: projects, memory, CPU, storage, and each enabled plugin's
    /// offerings by name.
    async fn meters(&self, tx: &DatabaseTransaction) -> Result<Vec<Value>> {
        let mut meters = vec![
            json!({"id":"projects","label":"Projects","unit":"count"}),
            json!({"id":"memory","label":"Memory","unit":"MiB"}),
            json!({"id":"cpu","label":"CPU","unit":"millicores"}),
            json!({"id":"storage","label":"Storage","unit":"MiB"}),
        ];
        let ids = tx.query_all_raw(Statement::from_string(DbBackend::Postgres,"SELECT id FROM webdock_auth.plugin_installation WHERE kind='external' AND state IN ('enabled','draining') ORDER BY id")).await.map_err(db_error)?;
        for row in ids {
            let plugin: String = row.try_get("", "id").map_err(db_error)?;
            let Ok(metadata) = self.metadata(tx, &plugin, None).await else {
                continue;
            };
            for offering in metadata.manifest.as_json()["offerings"]
                .as_array()
                .into_iter()
                .flatten()
            {
                if let Some(id) = offering["id"].as_str() {
                    meters.push(json!({"id": format!("{plugin}.{id}"), "label": offering["label"], "plugin": plugin, "unit": "count"}));
                }
            }
        }
        Ok(meters)
    }
    /// Operators: every plan with how many customers it has, and the meters plans can limit.
    pub async fn plans(&self, actor: &Principal, authority: Authority<'_>) -> Result<Value> {
        let guard = self.admin_guard(actor, authority, false).await?;
        let tx = self.operator_tx(actor).await?;
        let rows = tx.query_all_raw(Statement::from_string(DbBackend::Postgres,"SELECT jsonb_build_object('id',p.id,'name',p.name,'description',p.description,'limits',p.limits,'isDefault',p.is_default,'updatedAt',p.updated_at,'customers',(SELECT count(*) FROM webdock_auth.customer_plan c WHERE c.plan_id=p.id)) AS value FROM webdock_auth.plan p ORDER BY p.name,p.id")).await.map_err(db_error)?;
        let plans = rows
            .iter()
            .map(|r| r.try_get::<Value>("", "value").map_err(db_error))
            .collect::<Result<Vec<_>>>()?;
        let meters = self.meters(&tx).await?;
        self.admin_commit(tx, actor, authority, guard, false)
            .await?;
        Ok(json!({"plans": plans, "meters": meters}))
    }
    /// Create (`id` None) or replace a plan.
    pub async fn save_plan(
        &self,
        actor: &Principal,
        authority: Authority<'_>,
        id: Option<&str>,
        input: PlanInput,
    ) -> Result<Value> {
        if id.is_some_and(|id| !record(id)) {
            return Err(invalid("Invalid plan"));
        }
        let guard = self.admin_guard(actor, authority, true).await?;
        let tx = self.operator_tx(actor).await?;
        let id = match id {
            Some(id) => id.to_owned(),
            None => self
                .ids
                .allocate()
                .await
                .map_err(|_| unavailable())?
                .to_string(),
        };
        let name = input.name.trim().to_owned();
        if name.is_empty() {
            return Err(invalid(
                "Enter a name of up to 80 characters and a description of up to 500",
            ));
        }
        if input.is_default {
            tx.execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE webdock_auth.plan SET is_default=false WHERE is_default AND id<>$1",
                [id.clone().into()],
            ))
            .await
            .map_err(db_error)?;
        }
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "INSERT INTO webdock_auth.plan(id,name,description,limits,is_default,updated_by) VALUES($1,$2,$3,$4,$5,$6) \
             ON CONFLICT (id) DO UPDATE SET name=EXCLUDED.name,description=EXCLUDED.description,limits=EXCLUDED.limits,is_default=EXCLUDED.is_default,updated_by=EXCLUDED.updated_by,updated_at=now()",
            [id.clone().into(), name.into(), input.description.trim().to_owned().into(), input.limits.into(), input.is_default.into(), actor.user_id.clone().into()])).await.map_err(plan_error)?;
        self.audit_admin(&tx, actor, "plan.save", &id).await?;
        self.admin_commit(tx, actor, authority, guard, true).await?;
        Ok(json!({"id": id}))
    }
    pub async fn delete_plan(
        &self,
        actor: &Principal,
        authority: Authority<'_>,
        id: &str,
    ) -> Result<Value> {
        if !record(id) {
            return Err(invalid("Invalid plan"));
        }
        let guard = self.admin_guard(actor, authority, true).await?;
        let tx = self.operator_tx(actor).await?;
        let deleted = tx
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM webdock_auth.plan WHERE id=$1",
                [id.into()],
            ))
            .await
            .map_err(plan_error)?;
        if deleted.rows_affected() == 0 {
            return Err(PluginError::new(
                webdock_plugin_protocol::ErrorCode::NotFound,
                "Plan not found",
            ));
        }
        self.audit_admin(&tx, actor, "plan.delete", id).await?;
        self.admin_commit(tx, actor, authority, guard, true).await?;
        Ok(json!({}))
    }
    /// Assign a plan (None: the default plan) and per-meter overrides to a customer.
    pub async fn set_customer_plan(
        &self,
        actor: &Principal,
        authority: Authority<'_>,
        customer: &str,
        input: CustomerPlanInput,
    ) -> Result<Value> {
        if !record(customer) || input.plan_id.as_deref().is_some_and(|p| !record(p)) {
            return Err(invalid("Invalid plan assignment"));
        }
        let guard = self.admin_guard(actor, authority, true).await?;
        let tx = self.operator_tx(actor).await?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "INSERT INTO webdock_auth.customer_plan(customer_id,plan_id,overrides,updated_by) VALUES($1,$2,$3,$4) \
             ON CONFLICT (customer_id) DO UPDATE SET plan_id=EXCLUDED.plan_id,overrides=EXCLUDED.overrides,updated_by=EXCLUDED.updated_by,updated_at=now()",
            [customer.into(), input.plan_id.into(), input.overrides.into(), actor.user_id.clone().into()])).await.map_err(|e| {
                let text = e.to_string();
                if text.contains("customer_plan_customer_id_fkey") || text.contains("customer_plan_plan_id_fkey") {
                    PluginError::new(webdock_plugin_protocol::ErrorCode::NotFound, "Customer or plan not found")
                } else {
                    plan_error(e)
                }
            })?;
        self.audit_admin(&tx, actor, "customer.plan", customer)
            .await?;
        self.admin_commit(tx, actor, authority, guard, true).await?;
        Ok(json!({}))
    }
    /// A customer's plan, limits, allocation, units with measured use and 7 days of history, for
    /// the customer's members and operators.
    pub async fn usage(&self, actor: &Principal, customer: &str) -> Result<Value> {
        if !record(customer) {
            return Err(invalid("Invalid customer"));
        }
        let tx = self
            .signer
            .begin(&self.db, actor)
            .await
            .map_err(|_| PluginError::denied())?;
        timeouts(&tx).await?;
        let row = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.customer_usage($1) AS value",
                [customer.into()],
            ))
            .await
            .map_err(|_| PluginError::denied())?
            .ok_or_else(PluginError::denied)?;
        let mut usage: Value = row.try_get("", "value").map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        let controller = self.controller().await?;
        usage["meters"] = json!(self.meters(&controller).await?);
        controller.commit().await.map_err(db_error)?;
        Ok(usage)
    }
}
