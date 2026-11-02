//! Confidential native send queue operations. All state, quota and Webmail journal changes
//! commit together under the retained narrow signed worker scope.
use crate::{
    Error, MailDependencies, Result,
    authority::{self, Purpose},
    budget::{self, Settlement},
    keys::NativeKeys,
    outbox::{self, PayloadLease, Receipt, SendLease, field, one, run},
    proof::valid_id,
};
use sea_orm::DatabaseTransaction;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(tag = "action", deny_unknown_fields)]
pub enum QueueCommand {
    #[serde(rename = "send.status")]
    Status {
        token: String,
        id: String,
        #[serde(default)]
        cancel: bool,
    },
    #[serde(rename = "send.claim")]
    Claim {},
    #[serde(rename = "send.commitPayload")]
    CommitPayload { lease: PayloadLease },
    #[serde(rename = "send.abortPayload")]
    AbortPayload { lease: PayloadLease },
    #[serde(rename = "send.reviewPreparation")]
    ReviewPreparation { lease: SendLease },
    #[serde(rename = "send.finish")]
    Finish { lease: SendLease, result: Receipt },
    #[serde(rename = "send.cleanup")]
    Cleanup {},
    #[serde(rename = "send.cleanupDone")]
    CleanupDone { items: Vec<CleanupItem> },
}
pub use webdock_mail_protocol::outbox::CleanupItem;
pub struct Submissions {
    dependencies: MailDependencies,
    secret: String,
    enabled: bool,
}
impl Submissions {
    pub async fn new(dependencies: MailDependencies, secret: &str, enabled: bool) -> Result<Self> {
        authority::validate_pools(&dependencies).await?;
        if !(32..=4096).contains(&secret.len()) {
            return Err(Error::Invalid);
        }
        Ok(Self {
            dependencies,
            secret: secret.into(),
            enabled,
        })
    }
    pub async fn execute(&self, command: QueueCommand) -> Result<Value> {
        let admission = matches!(
            command,
            QueueCommand::Claim {} | QueueCommand::Status { .. }
        );
        if admission && !self.enabled {
            return Err(Error::Unavailable);
        }
        let tx = authority::worker(
            &self.dependencies,
            &self.secret,
            Purpose::Sending,
            admission.then_some(false),
        )
        .await?;
        let result = self.execute_in(&tx, command, None).await?;
        tx.commit().await.map_err(|_| Error::Unavailable)?;
        Ok(result)
    }
    pub(crate) async fn execute_in(
        &self,
        tx: &DatabaseTransaction,
        command: QueueCommand,
        lane: Option<u64>,
    ) -> Result<Value> {
        crate::managed_control::validate_queue_lane(tx, &command, lane).await?;
        let result = match command {
            QueueCommand::Status { token, id, cancel } => {
                if !valid_id(&id) {
                    return Err(Error::Invalid);
                }
                let access = NativeKeys::authenticate_in(tx, &token, None).await?;
                let row=one(tx,"SELECT id,state,payload_ref::text FROM webdock_mail.send_job WHERE id=$1 AND customer_id=$2 AND source_id=$3",vec![id.clone().into(),access.customer_id.clone().into(),format!("key:{}",access.key_id).into()]).await?.ok_or(Error::Unavailable)?;
                let mut state: String = field(&row, "state")?;
                if cancel {
                    completion_lock(tx, &access.customer_id).await?;
                    if outbox::cancel(tx, &access.customer_id, &id).await? {
                        budget::settle(tx, &access.customer_id, &id, Settlement::Released).await?;
                        state = "cancelled".into();
                    } else {
                        state=field(&one(tx,"SELECT state FROM webdock_mail.send_job WHERE id=$1 AND customer_id=$2",vec![id.clone().into(),access.customer_id.clone().into()]).await?.ok_or(Error::Unavailable)?,"state")?;
                    }
                }
                let mut result = json!({"id":id,"state":state});
                if state == "cancelled" {
                    result["cleanupRef"] = json!(field::<String>(&row, "payload_ref")?);
                }
                result
            }
            QueueCommand::Claim {} => {
                reap(tx, lane).await?;
                serde_json::to_value(outbox::claim_lane(tx, None, lane).await?)
                    .map_err(|_| Error::Unavailable)?
            }
            QueueCommand::CommitPayload { lease } => {
                if !outbox::commit_payload(tx, &lease).await? {
                    return Err(Error::Unavailable);
                }
                settle_journal(tx, &lease.id, "queued").await?;
                json!({"queued":true})
            }
            QueueCommand::AbortPayload { lease } => {
                check_payload(&lease)?;
                completion_lock(tx, &lease.customer_id).await?;
                let changed=run(tx,"UPDATE webdock_mail.send_job SET state='rejected',finished_at=now() WHERE id=$1 AND customer_id=$2 AND payload_ref=$3::uuid AND lease_token=$4::uuid AND state='staging'",vec![lease.id.clone().into(),lease.customer_id.clone().into(),lease.payload_ref.into(),lease.upload_token.into()]).await?;
                if changed {
                    budget::settle(tx, &lease.customer_id, &lease.id, Settlement::Released).await?;
                    settle_journal(tx, &lease.id, "rejected").await?;
                }
                json!({"aborted":changed})
            }
            QueueCommand::ReviewPreparation { lease } => {
                completion_lock(tx, &lease.customer_id).await?;
                json!(outbox::review_preparation(tx, &lease).await?)
            }
            QueueCommand::Finish { lease, result } => {
                completion_lock(tx, &lease.customer_id).await?;
                let changed = outbox::finish_handoff(tx, &lease, &result).await?;
                if changed {
                    settle_journal(tx, &lease.id, result.state()).await?;
                    match result {
                        Receipt::Unknown {} => {}
                        Receipt::Accepted { .. } => {
                            budget::settle(tx, &lease.customer_id, &lease.id, Settlement::Accepted)
                                .await?
                        }
                        Receipt::Rejected {} => {
                            budget::settle(tx, &lease.customer_id, &lease.id, Settlement::Released)
                                .await?
                        }
                    }
                }
                json!(changed)
            }
            QueueCommand::Cleanup {} => {
                let rows=tx.query_all_raw(sea_orm::Statement::from_sql_and_values(sea_orm::DbBackend::Postgres,"SELECT id,customer_id,payload_ref::text AS reference FROM webdock_mail.send_job WHERE (($1::bigint IS NULL AND source_context->>'executor' IS DISTINCT FROM 'managed-mail-v1') OR ($1 IS NOT NULL AND source_context->>'executor'='managed-mail-v1' AND source_context->>'payloadStoreID'='mail:platform:v1' AND jsonb_typeof(source_context->'payloadStoreVersion')='number' AND source_context->>'payloadStoreVersion'='1')) AND state IN ('accepted','rejected','cancelled') AND finished_at<now()-interval '1 minute' AND payload_deleted_at IS NULL AND (payload_format='request' OR state<>'accepted' OR filing_state='complete') ORDER BY finished_at LIMIT 50",[lane.map(|n|n as i64).into()])).await.map_err(|_|Error::Unavailable)?;
                let mut items = Vec::new();
                for row in rows {
                    items.push(CleanupItem {
                        id: field(&row, "id")?,
                        customer_id: field(&row, "customer_id")?,
                        reference: field(&row, "reference")?,
                    });
                }
                serde_json::to_value(items).map_err(|_| Error::Unavailable)?
            }
            QueueCommand::CleanupDone { items } => {
                if items.len() > 50
                    || items.iter().any(|v| {
                        !valid_id(&v.id) || !valid_id(&v.customer_id) || !uuid(&v.reference)
                    })
                {
                    return Err(Error::Invalid);
                }
                for item in items {
                    run(tx,"UPDATE webdock_mail.send_job SET payload_deleted_at=now() WHERE id=$1 AND customer_id=$2 AND payload_ref=$3::uuid AND state IN ('accepted','rejected','cancelled') AND (payload_format='request' OR state<>'accepted' OR filing_state='complete')",vec![item.id.into(),item.customer_id.into(),item.reference.into()]).await?;
                }
                json!({"ok":true})
            }
        };
        Ok(result)
    }
}
use sea_orm::ConnectionTrait;
fn uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|v| v.to_string() == value)
}
fn check_payload(value: &PayloadLease) -> Result<()> {
    if !valid_id(&value.id)
        || !valid_id(&value.customer_id)
        || !uuid(&value.payload_ref)
        || !uuid(&value.upload_token)
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
pub(crate) async fn completion_lock(tx: &DatabaseTransaction, customer: &str) -> Result<()> {
    if !valid_id(customer) {
        return Err(Error::Invalid);
    }
    one(
        tx,
        "SELECT id FROM webdock_admin.customers WHERE id=$1 FOR SHARE",
        vec![customer.into()],
    )
    .await?
    .ok_or(Error::Unavailable)?;
    one(
        tx,
        "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
        vec![format!("mail-budget:{customer}").into()],
    )
    .await?;
    Ok(())
}
pub(crate) async fn settle_journal(tx: &DatabaseTransaction, id: &str, state: &str) -> Result<()> {
    let Some(row)=one(tx,"SELECT customer_id,source_id,source_context FROM webdock_mail.send_job WHERE id=$1 AND payload_format='mime'",vec![id.into()]).await?else{return Ok(());};
    let source: Value = field(&row, "source_context")?;
    let write = source["writeID"]
        .as_str()
        .filter(|s| valid_id(s))
        .ok_or(Error::Unavailable)?;
    let mailbox = source["mailboxID"]
        .as_str()
        .filter(|s| valid_id(s))
        .ok_or(Error::Unavailable)?;
    let subject = source["subject"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 256)
        .ok_or(Error::Unavailable)?;
    if field::<String>(&row, "source_id")? != format!("webmail:{write}") {
        return Err(Error::Unavailable);
    }
    let outcome = match state {
        "cancelled" | "rejected" => "rejected",
        "unknown" => "uncertain",
        _ => "succeeded",
    };
    run(tx,"UPDATE webdock_mail.webmail_write SET state=$5,result=$6,finished_at=now(),queue_preparing_until=NULL WHERE id=$1 AND customer_id=$2 AND mailbox_id=$3 AND subject=$4 AND action='submit'",vec![write.into(),field::<String>(&row,"customer_id")?.into(),mailbox.into(),subject.into(),outcome.into(),json!({"id":id,"state":state}).into()]).await?;
    Ok(())
}
async fn reap(tx: &DatabaseTransaction, lane: Option<u64>) -> Result<()> {
    // An abandoned draft export is safe to reject only before it created any queued job.
    run(tx,"WITH abandoned AS (SELECT w.id FROM webdock_mail.webmail_write w WHERE w.state='reserved' AND w.queue_preparing_until<now() AND NOT EXISTS(SELECT 1 FROM webdock_mail.send_job j WHERE j.id=w.id) ORDER BY w.queue_preparing_until FOR UPDATE OF w SKIP LOCKED LIMIT 25) UPDATE webdock_mail.webmail_write SET state='rejected',queue_preparing_until=NULL WHERE id IN (SELECT id FROM abandoned)",vec![]).await?;
    let expired=tx.query_all_raw(sea_orm::Statement::from_sql_and_values(sea_orm::DbBackend::Postgres,"SELECT id,customer_id FROM webdock_mail.send_job WHERE state='staging' AND lease_until<now() AND (($1::bigint IS NULL AND source_context->>'executor' IS DISTINCT FROM 'managed-mail-v1') OR ($1 IS NOT NULL AND source_context->>'executor'='managed-mail-v1' AND source_context->>'payloadStoreID'='mail:platform:v1' AND jsonb_typeof(source_context->'payloadStoreVersion')='number' AND source_context->>'payloadStoreVersion'='1' AND source_context->>'executorGeneration'=$1::text)) ORDER BY customer_id,id LIMIT 25",[lane.map(|n|n as i64).into()])).await.map_err(|_|Error::Unavailable)?;
    for row in expired {
        let id: String = field(&row, "id")?;
        let customer: String = field(&row, "customer_id")?;
        completion_lock(tx, &customer).await?;
        if run(tx,"UPDATE webdock_mail.send_job SET state='rejected',finished_at=now() WHERE id=$1 AND state='staging' AND lease_until<now()",vec![id.clone().into()]).await?{budget::settle(tx,&customer,&id,Settlement::Released).await?;settle_journal(tx,&id,"rejected").await?;}
    }
    Ok(())
}
