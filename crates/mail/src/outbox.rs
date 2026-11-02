//! Durable send state transitions over the retained schema. Every function requires the caller's
//! existing authorized transaction. Admission, budget and source authorization belong in that same
//! transaction; handoff MUST commit before any transport I/O. These are not public HTTP handlers.
use crate::{Error, Result, proof::valid_id};
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, QueryResult, Statement};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

pub use webdock_mail_protocol::outbox::{PayloadFormat, PayloadLease, Receipt, SendLease};
#[derive(Clone)]
pub struct Enqueue {
    /// Already allocated and committed by the coordinated native ID allocator.
    pub id: String,
    pub customer_id: String,
    pub source_id: String,
    pub idempotency_key: String,
    pub request_hash: String,
    pub payload_ref: String,
    pub payload_bytes: i32,
    pub route: Value,
    pub envelope: Value,
    pub payload_format: PayloadFormat,
    pub source_context: Value,
    pub staging: bool,
    pub not_before: Option<DateTime<Utc>>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Enqueued {
    pub id: String,
    pub state: String,
    pub created: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_token: Option<String>,
}
pub(crate) async fn one(
    tx: &DatabaseTransaction,
    sql: &str,
    values: Vec<sea_orm::Value>,
) -> Result<Option<QueryResult>> {
    tx.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .map_err(|_| Error::Unavailable)
}
pub(crate) async fn run(
    tx: &DatabaseTransaction,
    sql: &str,
    values: Vec<sea_orm::Value>,
) -> Result<bool> {
    Ok(tx
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .await
        .map_err(|_| Error::Unavailable)?
        .rows_affected()
        > 0)
}
pub(crate) fn field<T: sea_orm::TryGetable>(row: &QueryResult, name: &str) -> Result<T> {
    row.try_get("", name).map_err(|_| Error::Unavailable)
}
fn text_token(s: &str) -> bool {
    !s.is_empty() && s.len() <= 200 && s.bytes().all(|b| (0x21..=0x7e).contains(&b))
}
fn uuid(s: &str) -> bool {
    Uuid::parse_str(s).is_ok_and(|u| u.to_string() == s)
}
fn hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn check_lease(job: &SendLease) -> Result<()> {
    if !valid_id(&job.id) || !valid_id(&job.customer_id) || !uuid(&job.lease_token) {
        return Err(Error::Invalid);
    }
    Ok(())
}
pub async fn enqueue(tx: &DatabaseTransaction, input: &Enqueue) -> Result<Enqueued> {
    if webdock_core::id::RecordId::parse(&input.id).is_err()
        || !valid_id(&input.customer_id)
        || !text_token(&input.source_id)
        || !text_token(&input.idempotency_key)
        || !hash(&input.request_hash)
        || !uuid(&input.payload_ref)
        || !(1..=26_214_400).contains(&input.payload_bytes)
        || !matches!(
            input.route.get("kind").and_then(Value::as_str),
            Some("direct" | "stalwart")
        )
        || !input.route.is_object()
        || input.route.to_string().len() > 4096
        || !input.envelope.is_object()
        || input.envelope.to_string().len() > 65536
        || !input.source_context.is_object()
        || input.source_context.to_string().len() > 4096
        || input
            .not_before
            .is_some_and(|t| t > Utc::now() + chrono::Duration::days(30))
    {
        return Err(Error::Invalid);
    }
    one(
        tx,
        "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
        vec![format!("mail-outbox:{}", input.customer_id).into()],
    )
    .await?;
    if let Some(row)=one(tx,"SELECT id,request_hash,state FROM webdock_mail.send_job WHERE customer_id=$1 AND source_id=$2 AND idempotency_key=$3",vec![input.customer_id.clone().into(),input.source_id.clone().into(),input.idempotency_key.clone().into()]).await?{
        if field::<String>(&row,"request_hash")?!=input.request_hash{return Err(Error::Invalid);}
        return Ok(Enqueued{id:field(&row,"id")?,state:field(&row,"state")?,created:false,upload_token:None});
    }
    let usage=one(tx,"SELECT count(*)::bigint AS count,coalesce(sum(payload_bytes),0)::bigint AS bytes FROM webdock_mail.send_job WHERE customer_id=$1 AND (state IN ('staging','queued','leased','preparation_review','handoff','unknown') OR (state='accepted' AND payload_format='mime' AND filing_state IS DISTINCT FROM 'complete'))",vec![input.customer_id.clone().into()]).await?.ok_or(Error::Unavailable)?;
    if field::<i64>(&usage, "count")? >= 1000
        || field::<i64>(&usage, "bytes")? + i64::from(input.payload_bytes) > 268_435_456
    {
        return Err(Error::Unavailable);
    }
    let token = input.staging.then(|| Uuid::new_v4().to_string());
    let state = if input.staging { "staging" } else { "queued" };
    run(tx,"INSERT INTO webdock_mail.send_job(id,customer_id,source_id,idempotency_key,request_hash,payload_ref,payload_bytes,route,not_before,envelope,state,lease_token,lease_until,payload_format,source_context,filing_state) VALUES($1,$2,$3,$4,$5,$6::uuid,$7,$8,coalesce($9,now()),$10,$11,$12::uuid,CASE WHEN $12::uuid IS NOT NULL THEN now()+interval '2 minutes' ELSE NULL END,$13,$14,CASE WHEN $13='mime' THEN 'pending' ELSE NULL END)",vec![input.id.clone().into(),input.customer_id.clone().into(),input.source_id.clone().into(),input.idempotency_key.clone().into(),input.request_hash.clone().into(),input.payload_ref.clone().into(),input.payload_bytes.into(),input.route.clone().into(),input.not_before.into(),input.envelope.clone().into(),state.into(),token.clone().into(),input.payload_format.as_str().into(),input.source_context.clone().into()]).await?;
    Ok(Enqueued {
        id: input.id.clone(),
        state: state.into(),
        created: true,
        upload_token: token,
    })
}
/// Optional tenant partition only narrows the existing signed worker authority.
pub async fn claim(tx: &DatabaseTransaction, customer: Option<&str>) -> Result<Option<SendLease>> {
    claim_lane(tx, customer, None).await
}
pub(crate) async fn claim_lane(
    tx: &DatabaseTransaction,
    customer: Option<&str>,
    lane: Option<u64>,
) -> Result<Option<SendLease>> {
    if customer.is_some_and(|s| !valid_id(s)) {
        return Err(Error::Invalid);
    }
    run(tx,"WITH expired AS (SELECT id FROM webdock_mail.send_job WHERE state IN ('leased','handoff') AND lease_until<now() AND ($1::text IS NULL OR customer_id=$1) AND (($2::bigint IS NULL AND source_context->>'executor' IS DISTINCT FROM 'managed-mail-v1') OR ($2 IS NOT NULL AND source_context->>'executor'='managed-mail-v1' AND source_context->>'payloadStoreID'='mail:platform:v1' AND jsonb_typeof(source_context->'payloadStoreVersion')='number' AND source_context->>'payloadStoreVersion'='1' AND source_context->>'executorGeneration'=$2::text)) FOR UPDATE SKIP LOCKED LIMIT 100) UPDATE webdock_mail.send_job SET state=CASE WHEN state='leased' THEN 'queued' ELSE 'unknown' END WHERE id IN (SELECT id FROM expired)",vec![customer.into(),lane.map(|n|n as i64).into()]).await?;
    let Some(row)=one(tx,"SELECT id,customer_id,source_id,payload_ref::text,payload_bytes,request_hash,payload_format,source_context,route,envelope FROM webdock_mail.send_job WHERE state='queued' AND not_before<=now() AND ($1::text IS NULL OR customer_id=$1) AND (($2::bigint IS NULL AND source_context->>'executor' IS DISTINCT FROM 'managed-mail-v1') OR ($2 IS NOT NULL AND source_context->>'executor'='managed-mail-v1' AND source_context->>'payloadStoreID'='mail:platform:v1' AND jsonb_typeof(source_context->'payloadStoreVersion')='number' AND source_context->>'payloadStoreVersion'='1' AND source_context->>'executorGeneration'=$2::text)) ORDER BY not_before,id FOR UPDATE SKIP LOCKED LIMIT 1",vec![customer.into(),lane.map(|n|n as i64).into()]).await?else{return Ok(None);};
    let id: String = field(&row, "id")?;
    let token = Uuid::new_v4().to_string();
    run(tx,"UPDATE webdock_mail.send_job SET state='leased',lease_token=$2::uuid,lease_until=now()+interval '2 minutes' WHERE id=$1",vec![id.clone().into(),token.clone().into()]).await?;
    Ok(Some(SendLease {
        id,
        customer_id: field(&row, "customer_id")?,
        source_id: field(&row, "source_id")?,
        payload_ref: field(&row, "payload_ref")?,
        payload_bytes: field(&row, "payload_bytes")?,
        request_hash: field(&row, "request_hash")?,
        payload_format: match field::<String>(&row, "payload_format")?.as_str() {
            "request" => PayloadFormat::Request,
            "mime" => PayloadFormat::Mime,
            _ => return Err(Error::Unavailable),
        },
        source_context: field(&row, "source_context")?,
        route: field(&row, "route")?,
        envelope: field(&row, "envelope")?,
        lease_token: token,
    }))
}
pub async fn begin_handoff(tx: &DatabaseTransaction, job: &SendLease) -> Result<bool> {
    check_lease(job)?;
    run(tx,"UPDATE webdock_mail.send_job SET state='handoff',lease_until=now()+interval '2 minutes' WHERE id=$1 AND customer_id=$2 AND lease_token=$3::uuid AND state='leased' AND lease_until>now()",vec![job.id.clone().into(),job.customer_id.clone().into(),job.lease_token.clone().into()]).await
}
pub async fn finish_handoff(
    tx: &DatabaseTransaction,
    job: &SendLease,
    result: &Receipt,
) -> Result<bool> {
    check_lease(job)?;
    if result.provider().is_some_and(|s| {
        s.is_empty()
            || s.len() > 255
            || !s
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    }) {
        return Err(Error::Invalid);
    }
    let Some(row)=one(tx,"SELECT state,provider_id FROM webdock_mail.send_job WHERE id=$1 AND customer_id=$2 AND lease_token=$3::uuid FOR UPDATE",vec![job.id.clone().into(),job.customer_id.clone().into(),job.lease_token.clone().into()]).await?else{return Ok(false);};
    let state: String = field(&row, "state")?;
    if state == result.state()
        && field::<Option<String>>(&row, "provider_id")?.as_deref() == result.provider()
    {
        return Ok(true);
    }
    if !["handoff", "unknown"].contains(&state.as_str()) {
        return Ok(false);
    }
    run(tx,"UPDATE webdock_mail.send_job SET state=$2,provider_id=$3,finished_at=CASE WHEN $2='unknown' THEN NULL ELSE now() END WHERE id=$1",vec![job.id.clone().into(),result.state().into(),result.provider().into()]).await
}
pub async fn cancel(tx: &DatabaseTransaction, customer: &str, id: &str) -> Result<bool> {
    if !valid_id(customer) || !valid_id(id) {
        return Err(Error::Invalid);
    }
    run(tx,"UPDATE webdock_mail.send_job SET state='cancelled',finished_at=now() WHERE id=$1 AND customer_id=$2 AND state IN ('staging','queued','leased','preparation_review')",vec![id.into(),customer.into()]).await
}
pub async fn review_preparation(tx: &DatabaseTransaction, job: &SendLease) -> Result<bool> {
    check_lease(job)?;
    run(tx,"UPDATE webdock_mail.send_job SET state='preparation_review',finished_at=NULL WHERE id=$1 AND customer_id=$2 AND lease_token=$3::uuid AND state='leased' AND lease_until>now()",vec![job.id.clone().into(),job.customer_id.clone().into(),job.lease_token.clone().into()]).await
}
pub async fn commit_payload(tx: &DatabaseTransaction, lease: &PayloadLease) -> Result<bool> {
    if !valid_id(&lease.id)
        || !valid_id(&lease.customer_id)
        || !uuid(&lease.upload_token)
        || !uuid(&lease.payload_ref)
    {
        return Err(Error::Invalid);
    }
    run(tx,"UPDATE webdock_mail.send_job SET state='queued',lease_token=NULL,lease_until=NULL WHERE id=$1 AND customer_id=$2 AND payload_ref=$3::uuid AND lease_token=$4::uuid AND state='staging' AND lease_until>now()",vec![lease.id.clone().into(),lease.customer_id.clone().into(),lease.payload_ref.clone().into(),lease.upload_token.clone().into()]).await
}
