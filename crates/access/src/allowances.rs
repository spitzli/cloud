//! Installation operator supplements; paid base/projection and usage are never rewritten.
use crate::{
    AccessService, Error, Result,
    contracts::id,
    service::{lock_session_context, operator},
};
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::future::Future;
use webdock_identity::Principal;

const MAX_SAFE: u64 = 9_007_199_254_740_991;
const DIMENSIONS: [&str; 7] = [
    "apps",
    "cpuMillicores",
    "memoryBytes",
    "ephemeralBytes",
    "volumeBytes",
    "replicasPerApp",
    "concurrentDeployments",
];

/// Core implements this using its real first-party browser authority. The supplied auth
/// transaction is retained through the domain commit. This avoids a browser/Access cycle.
pub trait AllowanceAuthority: Send + Sync {
    fn revalidate(
        &self,
        auth: &DatabaseTransaction,
    ) -> impl Future<Output = Result<Principal>> + Send;
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct FiniteAllowance(u64);
impl TryFrom<u64> for FiniteAllowance {
    type Error = &'static str;
    fn try_from(value: u64) -> std::result::Result<Self, Self::Error> {
        if value > MAX_SAFE {
            return Err("Allowance exceeds supported range");
        }
        Ok(Self(value))
    }
}
impl From<FiniteAllowance> for u64 {
    fn from(value: FiniteAllowance) -> Self {
        value.0
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostingAllowances {
    pub apps: FiniteAllowance,
    pub cpu_millicores: FiniteAllowance,
    pub memory_bytes: FiniteAllowance,
    pub ephemeral_bytes: FiniteAllowance,
    pub volume_bytes: FiniteAllowance,
    pub replicas_per_app: FiniteAllowance,
    pub concurrent_deployments: FiniteAllowance,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigureAllowances {
    pub expected_revision: u32,
    pub idempotency_key: String,
    pub hosting: HostingAllowances,
    pub mail_messages: FiniteAllowance,
}
impl ConfigureAllowances {
    fn validate(&self) -> Result<()> {
        if self.expected_revision >= i32::MAX as u32
            || self.idempotency_key.is_empty()
            || self.idempotency_key.len() > 128
            || !self
                .idempotency_key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
        {
            return Err(Error::InvalidInput);
        }
        Ok(())
    }
    fn supplements(&self) -> Value {
        json!({"hosting": self.hosting, "mailMessages": self.mail_messages})
    }
}

fn merge_extras(previous: &Value, input: &ConfigureAllowances) -> Result<Value> {
    let mut next = previous.as_object().cloned().ok_or(Error::Configuration)?;
    let mut hosting = match next.get("hosting") {
        None => serde_json::Map::new(),
        Some(value) => value.as_object().cloned().ok_or(Error::Configuration)?,
    };
    let supplied = input.supplements();
    for dimension in DIMENSIONS {
        hosting.insert(dimension.into(), supplied["hosting"][dimension].clone());
    }
    next.insert("hosting".into(), Value::Object(hosting));
    next.insert("mailMessages".into(), supplied["mailMessages"].clone());
    Ok(Value::Object(next))
}
fn units(value: Option<&Value>) -> Result<Option<u64>> {
    match value {
        None => Ok(Some(0)),
        Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .filter(|v| *v <= MAX_SAFE)
            .map(Some)
            .ok_or(Error::Configuration),
    }
}
fn sum(base: Option<&Value>, extra: Option<&Value>) -> Result<Value> {
    let extra = units(extra)?.ok_or(Error::Configuration)?;
    match units(base)? {
        None => Ok(Value::Null),
        Some(base) => base
            .checked_add(extra)
            .filter(|v| *v <= MAX_SAFE)
            .map(|v| json!(v))
            .ok_or(Error::InvalidInput),
    }
}
fn effective(base: &Value, extras: &Value) -> Result<Value> {
    let mut hosting = json!({});
    for key in DIMENSIONS {
        hosting[key] = sum(
            base.get("hosting").and_then(|v| v.get(key)),
            extras.get("hosting").and_then(|v| v.get(key)),
        )?;
    }
    Ok(
        json!({"hosting": hosting, "mailMessages": sum(base.get("mailMessages"), extras.get("mailMessages"))?}),
    )
}

async fn sql(tx: &DatabaseTransaction, query: &str, values: Vec<sea_orm::Value>) -> Result<()> {
    tx.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        query,
        values,
    ))
    .await?;
    Ok(())
}
async fn row(
    tx: &DatabaseTransaction,
    query: &str,
    values: Vec<sea_orm::Value>,
) -> Result<Option<sea_orm::QueryResult>> {
    Ok(tx
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            query,
            values,
        ))
        .await?)
}
fn installation_operator(actor: &Principal) -> Result<()> {
    if !actor.is_operator() || actor.customer_scope().is_some() {
        return Err(Error::Forbidden);
    }
    Ok(())
}
async fn scope(tx: &DatabaseTransaction, actor: &Principal, customer: &str) -> Result<()> {
    installation_operator(actor)?;
    operator(tx).await?;
    row(
        tx,
        "SELECT id FROM webdock_admin.users WHERE auth_subject=$1 AND role='operator' FOR SHARE",
        vec![actor.user_id.clone().into()],
    )
    .await?
    .ok_or(Error::Forbidden)?;
    // Match resource admission: customer before plan, then subscription; customer lock
    // also fences Mail reservations and customer deactivation without touching usage.
    row(tx, "SELECT c.id FROM webdock_admin.customers c JOIN webdock_auth.tenant_customer t ON t.customer_id=c.id WHERE c.id=$1 AND c.status='active' AND webdock_rls.native_organization_authority(t.organization_id) AND webdock_rls.customer_permission(c.id,'org:billing:manage') FOR UPDATE OF c FOR SHARE OF t", vec![id(customer)?.into()]).await?.ok_or(Error::NotFound)?;
    sql(
        tx,
        "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
        vec![format!("plan:{customer}").into()],
    )
    .await
}
async fn snapshot(tx: &DatabaseTransaction, customer: &str) -> Result<Value> {
    let subscription = row(tx, "SELECT base,extras,revision FROM webdock_auth.tenant_subscription WHERE customer_id=$1 FOR UPDATE", vec![customer.into()]).await?;
    let (base, extras, revision) = match subscription {
        Some(s) => (
            s.try_get::<Value>("", "base")?,
            s.try_get::<Value>("", "extras")?,
            s.try_get::<i32>("", "revision")?,
        ),
        None => (json!({}), json!({}), 0),
    };
    let supplements = effective(&json!({}), &extras)?;
    let subscription_effective = effective(&base, &extras)?;
    let mut values = subscription_effective.clone();
    if let Some(limit) = row(tx, "SELECT values FROM webdock_auth.hosting_limit WHERE customer_id=$1 AND scope='customer' FOR SHARE", vec![customer.into()]).await? {
        let limit: Value = limit.try_get("", "values")?;
        for key in DIMENSIONS {
            if let Some(ceiling) = units(limit.get(key))? {
                // Missing ceilings are unconstrained, unlike missing subscription values.
                if limit.get(key).is_some() {
                    values["hosting"][key] = json!(values["hosting"][key].as_u64().map_or(ceiling, |v| v.min(ceiling)));
                }
            }
        }
    }
    Ok(json!({"customerID": customer, "revision": revision,
        "supplements": supplements,
        "subscriptionEffective": subscription_effective, "effective": values}))
}
impl AccessService {
    async fn allowance_transactions<A: AllowanceAuthority>(
        &self,
        authority: &A,
    ) -> Result<(DatabaseTransaction, DatabaseTransaction, Principal)> {
        // Identification does not retain auth locks while another preview owns the fence.
        let identification = self.auth.begin().await?;
        let identified = authority.revalidate(&identification).await?;
        installation_operator(&identified)?;
        identification.commit().await?;
        let tx = self.signer.begin(&self.business, &identified).await?;
        lock_session_context(&tx, &identified.session_id).await?;
        let auth = self.auth.begin().await?;
        let actor = authority.revalidate(&auth).await?;
        installation_operator(&actor)?;
        if actor.user_id != identified.user_id || actor.session_id != identified.session_id {
            return Err(Error::Unauthorized);
        }
        Ok((tx, auth, actor))
    }
    /// The adapter must supply current server-owned Console proof, never a client DTO.
    pub async fn configure_allowances<A: AllowanceAuthority>(
        &self,
        authority: &A,
        customer: &str,
        input: ConfigureAllowances,
    ) -> Result<Value> {
        input.validate()?;
        let (tx, auth, actor) = self.allowance_transactions(authority).await?;
        scope(&tx, &actor, customer).await?;
        // Actor/key lock additionally serializes a key reused across different customers.
        sql(
            &tx,
            "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
            vec![format!("allowance:{}:{}", actor.user_id, input.idempotency_key).into()],
        )
        .await?;
        let payload = json!({"customerID": customer, "command": input});
        let digest = hex::encode(Sha256::digest(
            serde_json::to_vec(&payload).map_err(|_| Error::InvalidInput)?,
        ));
        let prior = row(&tx, "SELECT customer_id,payload_sha256,result FROM webdock_auth.allowance_operation WHERE actor_id=$1 AND idempotency_key=$2", vec![actor.user_id.clone().into(), input.idempotency_key.clone().into()]).await?;
        let result = if let Some(prior) = prior {
            if prior.try_get::<String>("", "customer_id")? != customer
                || prior.try_get::<String>("", "payload_sha256")? != digest
            {
                return Err(Error::Conflict);
            }
            prior.try_get::<Value>("", "result")?
        } else {
            self.write_allowances(&tx, &actor, customer, &input, &digest)
                .await?
        };
        let current = authority.revalidate(&auth).await?;
        installation_operator(&current)?;
        if current.user_id != actor.user_id || current.session_id != actor.session_id {
            return Err(Error::Unauthorized);
        }
        scope(&tx, &current, customer).await?;
        tx.commit().await?;
        auth.commit().await?;
        Ok(result)
    }
    pub async fn read_allowances<A: AllowanceAuthority>(
        &self,
        authority: &A,
        customer: &str,
    ) -> Result<Value> {
        let (tx, auth, actor) = self.allowance_transactions(authority).await?;
        scope(&tx, &actor, customer).await?;
        let mut result = snapshot(&tx, customer).await?;
        result["receipt"] = row(&tx, "SELECT jsonb_build_object('operationID',id,'revision',result->'revision') AS receipt FROM webdock_auth.allowance_operation WHERE customer_id=$1 AND actor_id=$2 ORDER BY created_at DESC,id DESC LIMIT 1", vec![customer.into(), actor.user_id.clone().into()]).await?.map(|r| r.try_get::<Value>("", "receipt")).transpose()?.unwrap_or(Value::Null);
        let current = authority.revalidate(&auth).await?;
        installation_operator(&current)?;
        if current.user_id != actor.user_id || current.session_id != actor.session_id {
            return Err(Error::Unauthorized);
        }
        scope(&tx, &current, customer).await?;
        tx.commit().await?;
        auth.commit().await?;
        Ok(result)
    }
    async fn write_allowances(
        &self,
        tx: &DatabaseTransaction,
        actor: &Principal,
        customer: &str,
        input: &ConfigureAllowances,
        digest: &str,
    ) -> Result<Value> {
        let old = row(tx, "SELECT extras,revision FROM webdock_auth.tenant_subscription WHERE customer_id=$1 FOR UPDATE", vec![customer.into()]).await?;
        let (extras, revision) = match old {
            Some(s) => (
                s.try_get::<Value>("", "extras")?,
                s.try_get::<i32>("", "revision")?,
            ),
            None => (json!({}), 0),
        };
        if i64::from(revision) != i64::from(input.expected_revision) {
            return Err(Error::Conflict);
        }
        let extras = merge_extras(&extras, input)?;
        if revision == 0 {
            // No manual base configuration: reject orphaned projected paid state.
            if row(tx, "SELECT c.id FROM webdock_auth.billing_contract c WHERE c.customer_id=$1 AND c.environment='live' AND c.projected_at IS NOT NULL AND NOT EXISTS(SELECT 1 FROM webdock_auth.billing_manual_release r WHERE r.contract_id=c.id)", vec![customer.into()]).await?.is_some() { return Err(Error::Conflict); }
            sql(tx, "INSERT INTO webdock_auth.tenant_subscription(customer_id,plan_id,name,description,base,extras,revision,updated_by) VALUES($1,NULL,'Operator allowances','','{}'::jsonb,$2,1,$3)", vec![customer.into(), extras.into(), actor.user_id.clone().into()]).await?;
        } else {
            row(tx, "UPDATE webdock_auth.tenant_subscription SET extras=$2,revision=revision+1,updated_by=$3,updated_at=statement_timestamp() WHERE customer_id=$1 AND revision=$4 RETURNING revision", vec![customer.into(), extras.into(), actor.user_id.clone().into(), revision.into()]).await?.ok_or(Error::Conflict)?;
        }
        sql(tx, "INSERT INTO webdock_auth.subscription_history(customer_id,revision,name,description,base,extras,plan_id,actor_id) SELECT customer_id,revision,name,description,base,extras,plan_id,updated_by FROM webdock_auth.tenant_subscription WHERE customer_id=$1", vec![customer.into()]).await?;
        let operation = self.next_id().await?;
        let mut result = snapshot(tx, customer).await?;
        result["receipt"] = json!({"operationID": operation, "revision": result["revision"]});
        sql(tx, "INSERT INTO webdock_auth.allowance_operation(id,actor_id,customer_id,idempotency_key,payload_sha256,result) VALUES($1,$2,$3,$4,$5,$6)", vec![operation.into(), actor.user_id.clone().into(), customer.into(), input.idempotency_key.clone().into(), digest.into(), result.clone().into()]).await?;
        self.audit(tx, actor, "allowances-configured", Some(customer), false)
            .await?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn command() -> Value {
        json!({"expectedRevision":0,"idempotencyKey":"native-grant-1", "hosting":{
            "apps":2,"cpuMillicores":1000,"memoryBytes":1073741824,"ephemeralBytes":2147483648u64,
            "volumeBytes":4294967296u64,"replicasPerApp":1,"concurrentDeployments":1},"mailMessages":10})
    }
    #[test]
    fn command_accepts_only_finite_typed_supplements() {
        assert!(
            serde_json::from_value::<ConfigureAllowances>(command())
                .unwrap()
                .validate()
                .is_ok()
        );
        for invalid in [
            Value::Null,
            json!(-1),
            json!(1.5),
            json!("unlimited"),
            json!(true),
            json!(MAX_SAFE + 1),
        ] {
            for path in ["mailMessages", "apps"] {
                let mut body = command();
                if path == "apps" {
                    body["hosting"][path] = invalid.clone();
                } else {
                    body[path] = invalid.clone();
                }
                assert!(
                    serde_json::from_value::<ConfigureAllowances>(body).is_err(),
                    "{path} {invalid}"
                );
            }
        }
        for field in [
            "role",
            "base",
            "planID",
            "paid",
            "projection",
            "description",
            "payment",
            "unlimited",
        ] {
            let mut body = command();
            body[field] = json!(true);
            assert!(
                serde_json::from_value::<ConfigureAllowances>(body).is_err(),
                "{field}"
            );
            let mut body = command();
            body["hosting"][field] = json!(1);
            assert!(serde_json::from_value::<ConfigureAllowances>(body).is_err());
        }
        let mut body = command();
        body["hosting"].as_object_mut().unwrap().remove("apps");
        assert!(serde_json::from_value::<ConfigureAllowances>(body).is_err());
        for key in ["", "unsafe key", "\n", &"x".repeat(129)] {
            let mut body = command();
            body["idempotencyKey"] = json!(key);
            assert!(
                serde_json::from_value::<ConfigureAllowances>(body)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        let mut body = command();
        body["expectedRevision"] = json!(i32::MAX);
        assert!(
            serde_json::from_value::<ConfigureAllowances>(body)
                .unwrap()
                .validate()
                .is_err()
        );
    }
    #[test]
    fn supplements_preserve_other_products_and_unlimited_paid_base() {
        let input: ConfigureAllowances = serde_json::from_value(command()).unwrap();
        let prior = json!({"projects":8,"databases":3,"hosting":{"retainedDimension":7,"apps":1}});
        let extras = merge_extras(&prior, &input).unwrap();
        assert_eq!(extras["projects"], 8);
        assert_eq!(extras["databases"], 3);
        assert_eq!(extras["hosting"]["retainedDimension"], 7);
        assert_eq!(extras["hosting"]["apps"], 2);
        assert_eq!(extras["mailMessages"], 10);
        let paid_base = json!({"mailMessages":null,"hosting":{"apps":null,"cpuMillicores":250}});
        let values = effective(&paid_base, &extras).unwrap();
        assert_eq!(values["mailMessages"], Value::Null);
        assert_eq!(values["hosting"]["apps"], Value::Null);
        assert_eq!(values["hosting"]["cpuMillicores"], 1250);
        let zero = effective(&json!({}), &json!({})).unwrap();
        assert_eq!(zero["hosting"]["apps"], 0);
        assert_eq!(zero["mailMessages"], 0);
        let fresh = effective(&json!({}), &extras).unwrap();
        assert_eq!(fresh["hosting"]["apps"], 2);
        assert_eq!(fresh["mailMessages"], 10);
        assert!(effective(&json!({"mailMessages":MAX_SAFE}), &extras).is_err());
        assert!(merge_extras(&json!({"hosting":null}), &input).is_err());
    }
}
