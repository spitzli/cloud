//! Native Mail and retained product usage share one recipient allowance and lock namespace.
use crate::{
    Error, Result,
    outbox::{field, one, run},
    proof::valid_id,
};
use sea_orm::DatabaseTransaction;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum SendPath {
    Direct {},
    Stalwart {
        #[serde(rename = "serviceRevision")]
        service_revision: String,
    },
}
pub struct Reservation {
    pub created: bool,
    pub state: String,
    pub path: SendPath,
}
#[derive(Clone, Copy)]
pub enum Settlement {
    Accepted,
    Released,
}
impl Settlement {
    fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Released => "released",
        }
    }
}
fn ids(customer: &str, reference: &str) -> Result<()> {
    if !valid_id(customer) || !valid_id(reference) {
        return Err(Error::Invalid);
    }
    Ok(())
}
fn units(value: Option<&Value>, default: u64) -> Result<Option<u64>> {
    match value {
        Some(Value::Null) => Ok(None),
        None => Ok(Some(default)),
        Some(v) => v
            .as_u64()
            .filter(|n| *n <= 9_007_199_254_740_991)
            .map(Some)
            .ok_or(Error::Unavailable),
    }
}
/// Internal primitive: the caller authorizes source, sender and provider binding in this transaction.
pub async fn reserve(
    tx: &DatabaseTransaction,
    customer: &str,
    reference: &str,
    count: i32,
) -> Result<Reservation> {
    ids(customer, reference)?;
    if !(1..=100).contains(&count) {
        return Err(Error::Invalid);
    }
    if one(tx,"SELECT c.id FROM webdock_admin.customers c WHERE c.id=$1 AND c.status='active' AND EXISTS(SELECT 1 FROM webdock_auth.platform_settings WHERE id=true AND settings->>'mailEnabled'='true') FOR SHARE OF c",vec![customer.into()]).await?.is_none(){return Err(Error::Unavailable);}
    // The customer lock fences concurrent incoming activation (which uses FOR UPDATE).
    let path=if let Some(incoming)=one(tx,"SELECT desired_enabled,state,revision,applied_revision FROM webdock_mail.service WHERE customer_id=$1 FOR SHARE",vec![customer.into()]).await?{
        let revision:i64=field(&incoming,"revision")?;
        if !field::<bool>(&incoming,"desired_enabled")?||field::<String>(&incoming,"state")?!="ready"||revision!=field::<i64>(&incoming,"applied_revision")?{return Err(Error::Unavailable);}
        SendPath::Stalwart{service_revision:revision.to_string()}
    }else{SendPath::Direct {}};
    one(
        tx,
        "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
        vec![format!("mail-budget:{customer}").into()],
    )
    .await?;
    let existing=one(tx,"SELECT customer_id,units,state,send_path,period_start::text AS period FROM webdock_mail.budget_reservation WHERE reference_id=$1",vec![reference.into()]).await?;
    if let Some(row) = &existing {
        let previous: SendPath =
            serde_json::from_value(field(row, "send_path")?).map_err(|_| Error::Unavailable)?;
        if field::<String>(row, "customer_id")? != customer
            || field::<i32>(row, "units")? != count
            || previous != path
        {
            return Err(Error::Unavailable);
        }
        let state: String = field(row, "state")?;
        if state != "reserved" {
            return Ok(Reservation {
                created: false,
                state,
                path,
            });
        }
    }
    let subscription = one(
        tx,
        "SELECT base,extras FROM webdock_auth.tenant_subscription WHERE customer_id=$1 FOR SHARE",
        vec![customer.into()],
    )
    .await?;
    let (base, extras) = if let Some(row) = subscription {
        (
            field::<Value>(&row, "base")?,
            field::<Value>(&row, "extras")?,
        )
    } else {
        (Value::Null, Value::Null)
    };
    let base = units(base.get("mailMessages"), 0)?;
    let extra = units(extras.get("mailMessages"), 0)?.ok_or(Error::Unavailable)?;
    let limit = base.map(|n| n + extra);
    if limit.is_some_and(|n| n > 9_007_199_254_740_991) {
        return Err(Error::Unavailable);
    }
    let period: String = field(
        &one(
            tx,
            "SELECT date_trunc('month',now() AT TIME ZONE 'UTC')::date::text AS period",
            vec![],
        )
        .await?
        .ok_or(Error::Unavailable)?,
        "period",
    )?;
    run(tx,"INSERT INTO webdock_mail.month_usage(customer_id,period_start) VALUES($1,$2::date) ON CONFLICT DO NOTHING",vec![customer.into(),period.clone().into()]).await?;
    let usage=one(tx,"SELECT reserved,accepted FROM webdock_mail.month_usage WHERE customer_id=$1 AND period_start=$2::date FOR UPDATE",vec![customer.into(),period.clone().into()]).await?.ok_or(Error::Unavailable)?;
    let previous_period = existing
        .as_ref()
        .map(|r| field::<String>(r, "period"))
        .transpose()?;
    let additional = if previous_period.as_deref() == Some(&period) {
        0
    } else {
        count
    };
    // The baseline always has this retained product table. Missing schema is not zero usage.
    let gateway:String=field(&one(tx,"SELECT coalesce(sum(reserved+consumed),0)::text AS used FROM webdock_auth.mail_usage_period WHERE customer_id=$1 AND period=$2::date",vec![customer.into(),period.clone().into()]).await?.ok_or(Error::Unavailable)?,"used")?;
    let used = i128::from(field::<i64>(&usage, "reserved")?)
        + i128::from(field::<i64>(&usage, "accepted")?)
        + gateway.parse::<i128>().map_err(|_| Error::Unavailable)?
        + i128::from(additional);
    if limit.is_some_and(|n| used > i128::from(n)) {
        return Err(Error::Unavailable);
    }
    if let Some(previous) = previous_period {
        if previous != period {
            run(tx,"UPDATE webdock_mail.month_usage SET reserved=reserved-$3 WHERE customer_id=$1 AND period_start=$2::date",vec![customer.into(),previous.into(),count.into()]).await?;
            run(tx,"UPDATE webdock_mail.month_usage SET reserved=reserved+$3 WHERE customer_id=$1 AND period_start=$2::date",vec![customer.into(),period.clone().into(),count.into()]).await?;
            run(tx,"UPDATE webdock_mail.budget_reservation SET period_start=$2::date WHERE reference_id=$1",vec![reference.into(),period.into()]).await?;
        }
        return Ok(Reservation {
            created: false,
            state: "reserved".into(),
            path,
        });
    }
    run(tx,"INSERT INTO webdock_mail.budget_reservation(reference_id,customer_id,period_start,units,send_path) VALUES($1,$2,$3::date,$4,$5)",vec![reference.into(),customer.into(),period.clone().into(),count.into(),serde_json::to_value(&path).map_err(|_|Error::Invalid)?.into()]).await?;
    run(tx,"UPDATE webdock_mail.month_usage SET reserved=reserved+$3 WHERE customer_id=$1 AND period_start=$2::date",vec![customer.into(),period.into(),count.into()]).await?;
    Ok(Reservation {
        created: true,
        state: "reserved".into(),
        path,
    })
}
/// Unknown acceptance has no settlement variant; retain the reservation until definitive evidence.
pub async fn settle(
    tx: &DatabaseTransaction,
    customer: &str,
    reference: &str,
    state: Settlement,
) -> Result<()> {
    ids(customer, reference)?;
    one(
        tx,
        "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
        vec![format!("mail-budget:{customer}").into()],
    )
    .await?;
    let row=one(tx,"SELECT units,period_start::text AS period,state FROM webdock_mail.budget_reservation WHERE customer_id=$1 AND reference_id=$2 FOR UPDATE",vec![customer.into(),reference.into()]).await?.ok_or(Error::Unavailable)?;
    let current: String = field(&row, "state")?;
    if current == state.as_str() {
        return Ok(());
    }
    if current != "reserved" {
        return Err(Error::Unavailable);
    }
    let count: i32 = field(&row, "units")?;
    run(tx,"UPDATE webdock_mail.month_usage SET reserved=reserved-$3,accepted=accepted+$4 WHERE customer_id=$1 AND period_start=$2::date",vec![customer.into(),field::<String>(&row,"period")?.into(),count.into(),if matches!(state,Settlement::Accepted){count}else{0}.into()]).await?;
    run(tx,"UPDATE webdock_mail.budget_reservation SET state=$3,settled_at=now() WHERE customer_id=$1 AND reference_id=$2",vec![customer.into(),reference.into(),state.as_str().into()]).await?;
    Ok(())
}
