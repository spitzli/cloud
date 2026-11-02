//! Durable Webmail intent: live sessions admit work; committed jobs retain current grant authority
//! across logout. Sent filing is a separate action and can never dispatch another message.
use crate::{
    Error, Result,
    authority::{self, Purpose},
    budget::{self, SendPath, Settlement},
    outbox::{self, Enqueue, PayloadFormat, SendLease, field, one, run},
    proof::valid_id,
    sending::{Envelope, Sending},
    webmail::{GatewayAccess, ReserveWrite, WebmailService, WriteAction},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use sea_orm::DatabaseTransaction;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn object_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 255
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebmailSource {
    #[serde(rename = "scopeID")]
    pub scope_id: String,
    pub grant_revision: String,
    pub subject: String,
    #[serde(rename = "mailboxID")]
    pub mailbox_id: String,
    #[serde(rename = "writeID")]
    pub write_id: String,
    #[serde(rename = "emailID")]
    pub email_id: String,
    #[serde(rename = "identityID")]
    pub identity_id: String,
    pub sender_name: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Prepare {
    pub token: String,
    #[serde(rename = "requestID")]
    pub request_id: uuid::Uuid,
    #[serde(rename = "emailID")]
    pub email_id: String,
    #[serde(rename = "identityID")]
    pub identity_id: String,
}
impl Prepare {
    fn validate(&self) -> Result<()> {
        if !object_id(&self.email_id) || !object_id(&self.identity_id) {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Stage {
    pub token: String,
    #[serde(rename = "requestID")]
    pub request_id: uuid::Uuid,
    #[serde(rename = "emailID")]
    pub email_id: String,
    #[serde(rename = "identityID")]
    pub identity_id: String,
    #[serde(rename = "operationID")]
    pub operation_id: String,
    pub lease_token: uuid::Uuid,
    pub payload_ref: String,
    pub payload_bytes: i32,
    pub request_hash: String,
    pub envelope: Envelope,
}
#[derive(Deserialize)]
#[serde(tag = "action", deny_unknown_fields)]
pub enum WebmailCommand {
    #[serde(rename = "send.webmail.prepare")]
    Prepare { submission: Prepare },
    #[serde(rename = "send.webmail.stage")]
    Stage { submission: Box<Stage> },
    #[serde(rename = "send.webmail.abort")]
    Abort {
        #[serde(rename = "operationID")]
        operation_id: String,
        #[serde(rename = "leaseToken")]
        lease_token: uuid::Uuid,
    },
    #[serde(rename = "send.webmail.status")]
    Status {
        token: String,
        id: String,
        #[serde(default)]
        cancel: bool,
    },
    #[serde(rename = "send.webmail.lookup")]
    Lookup {
        token: String,
        #[serde(rename = "emailID")]
        email_id: String,
    },
    #[serde(rename = "send.webmail.filingAccess")]
    FilingAccess {
        id: String,
        token: Option<String>,
        #[serde(rename = "customerID")]
        customer_id: Option<String>,
        #[serde(rename = "leaseToken")]
        lease_token: Option<uuid::Uuid>,
    },
    #[serde(rename = "send.webmail.filed")]
    Filed {
        id: String,
        #[serde(rename = "customerID")]
        customer_id: String,
        #[serde(rename = "leaseToken")]
        lease_token: uuid::Uuid,
        filed: bool,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueuedAccess {
    #[serde(rename = "customerID")]
    pub customer_id: String,
    #[serde(rename = "mailboxID")]
    pub mailbox_id: String,
    #[serde(rename = "accountID")]
    pub account_id: String,
    pub sender_name: String,
    pub sender_address: String,
    pub runtime_origin: String,
    pub authorization: String,
    pub subject: String,
}
impl Sending {
    pub fn with_webmail(mut self, service: std::sync::Arc<WebmailService>) -> Self {
        self.webmail = Some(service);
        self
    }
    pub async fn execute_webmail(&self, command: WebmailCommand) -> Result<Value> {
        let admission = matches!(
            command,
            WebmailCommand::Prepare { .. } | WebmailCommand::Stage { .. }
        );
        if admission && !self.config.enabled {
            return Err(Error::Unavailable);
        }
        let module = match command {
            WebmailCommand::Abort { .. } | WebmailCommand::Filed { .. } => None,
            _ => Some(admission),
        };
        let service = self.webmail.as_ref().ok_or(Error::Unavailable)?;
        let tx = authority::worker(
            &self.dependencies,
            &self.config.rls_secret,
            Purpose::Sending,
            module,
        )
        .await?;
        match command {
            WebmailCommand::Prepare { submission } => {
                self.prepare_webmail(tx, service, submission).await
            }
            WebmailCommand::Stage { submission } => {
                self.stage_webmail(tx, service, *submission).await
            }
            WebmailCommand::Status { token, id, cancel } => {
                let (access, authorized) = service.inspect_in(&tx, &token).await?;
                let result = status(&tx, &access, &id, cancel).await?;
                WebmailService::finish(tx, authorized).await?;
                Ok(result)
            }
            WebmailCommand::Lookup { token, email_id } => {
                if !object_id(&email_id) {
                    return Err(Error::Invalid);
                }
                let (access, authorized) = service.inspect_in(&tx, &token).await?;
                let row=one(&tx,"SELECT w.id,w.state AS write_state,j.state,j.filing_state FROM webdock_mail.webmail_write w LEFT JOIN webdock_mail.send_job j ON j.id=w.id AND j.customer_id=w.customer_id AND j.payload_format='mime' WHERE w.customer_id=$1 AND w.mailbox_id=$2 AND w.resource_id=$3 AND w.action='submit' ORDER BY w.created_at DESC,w.id DESC LIMIT 1",vec![access.customer_id.into(),access.mailbox_id.into(),email_id.into()]).await?;
                let result = row
                    .map(|r| status_row(&r))
                    .transpose()?
                    .unwrap_or(Value::Null);
                WebmailService::finish(tx, authorized).await?;
                Ok(result)
            }
            WebmailCommand::Abort {
                operation_id,
                lease_token,
            } => {
                if !valid_id(&operation_id) {
                    return Err(Error::Invalid);
                }
                run(&tx,"UPDATE webdock_mail.webmail_write w SET state='rejected',queue_preparing_until=NULL WHERE w.id=$1 AND w.lease_token=$2::uuid AND w.state='reserved' AND w.queue_preparing_until IS NOT NULL AND NOT EXISTS(SELECT 1 FROM webdock_mail.send_job j WHERE j.id=w.id)",vec![operation_id.into(),lease_token.to_string().into()]).await?;
                tx.commit().await.map_err(|_| Error::Unavailable)?;
                Ok(json!({"ok":true}))
            }
            WebmailCommand::FilingAccess {
                id,
                token,
                customer_id,
                lease_token,
            } => {
                self.filing_access(tx, service, &id, token, customer_id, lease_token)
                    .await
            }
            WebmailCommand::Filed {
                id,
                customer_id,
                lease_token,
                filed,
            } => {
                if !valid_id(&id) || !valid_id(&customer_id) {
                    return Err(Error::Invalid);
                }
                let row=one(&tx,"UPDATE webdock_mail.send_job SET filing_state=CASE WHEN filing_state='complete' THEN 'complete' ELSE $4 END WHERE id=$1 AND customer_id=$2 AND lease_token=$3::uuid AND state='accepted' AND payload_format='mime' RETURNING filing_state",vec![id.into(),customer_id.into(),lease_token.to_string().into(),if filed {"complete"} else {"needs_review"}.into()]).await?;
                let result = json!({"recorded":row.is_some(),"filed":row.map(|r|field::<String>(&r,"filing_state")).transpose()?.as_deref()==Some("complete")});
                tx.commit().await.map_err(|_| Error::Unavailable)?;
                Ok(result)
            }
        }
    }
    async fn prepare_webmail(
        &self,
        tx: DatabaseTransaction,
        service: &WebmailService,
        input: Prepare,
    ) -> Result<Value> {
        input.validate()?;
        let (access, authorized) = service.inspect_in(&tx, &input.token).await?;
        let payload_hash = hex::encode(Sha256::digest(
            serde_json::to_vec(&json!({"emailId":input.email_id,"identityId":input.identity_id}))
                .map_err(|_| Error::Invalid)?,
        ));
        let reservation = service
            .reserve_write(ReserveWrite {
                token: input.token,
                request_id: input.request_id,
                action: WriteAction::Submit,
                resource_id: Some(input.email_id),
                payload_hash,
            })
            .await?;
        let result = if reservation.execute {
            run(&tx,"UPDATE webdock_mail.webmail_write SET queue_preparing_until=now()+interval '2 minutes' WHERE id=$1 AND state='reserved'",vec![reservation.operation_id.clone().into()]).await?;
            json!({"existing":false,"operationID":reservation.operation_id,"leaseToken":reservation.lease_token,"access":access})
        } else {
            let mut result = status(&tx, &access, &reservation.operation_id, false).await?;
            result["existing"] = json!(true);
            result
        };
        WebmailService::finish(tx, authorized).await?;
        Ok(result)
    }
    async fn stage_webmail(
        &self,
        tx: DatabaseTransaction,
        service: &WebmailService,
        input: Stage,
    ) -> Result<Value> {
        input.envelope.validate()?;
        if !valid_id(&input.operation_id)
            || !object_id(&input.email_id)
            || !object_id(&input.identity_id)
        {
            return Err(Error::Invalid);
        }
        let (access, authorized) = service.inspect_in(&tx, &input.token).await?;
        if input.envelope.from != access.sender_address {
            return Err(Error::Unavailable);
        }
        let snapshot = self
            .policy
            .snapshot(&tx, &access.customer_id, false, None)
            .await?;
        super::sending::sender(&tx, &access.customer_id, &input.envelope.from).await?;
        let budget = budget::reserve(
            &tx,
            &access.customer_id,
            &input.operation_id,
            input.envelope.recipients(),
        )
        .await?;
        let SendPath::Stalwart { service_revision } = &budget.path else {
            return Err(Error::Unavailable);
        };
        if budget.state != "reserved" {
            return Err(Error::Unavailable);
        }
        self.config
            .incoming
            .as_ref()
            .ok_or(Error::Unavailable)?
            .connection(
                &tx,
                &access.customer_id,
                service_revision,
                snapshot.transport_revision,
            )
            .await?;
        let row=one(&tx,"SELECT payload_hash FROM webdock_mail.webmail_write WHERE id=$1 AND lease_token=$2::uuid AND customer_id=$3 AND mailbox_id=$4 AND subject=$5 AND request_id=$6::uuid AND resource_id=$7 AND action='submit' AND state='reserved' AND queue_preparing_until>now() FOR UPDATE",vec![input.operation_id.clone().into(),input.lease_token.to_string().into(),access.customer_id.clone().into(),access.mailbox_id.clone().into(),access.subject.clone().into(),input.request_id.to_string().into(),input.email_id.clone().into()]).await?.ok_or(Error::Unavailable)?;
        let hash = hex::encode(Sha256::digest(
            serde_json::to_vec(&json!({"emailId":input.email_id,"identityId":input.identity_id}))
                .map_err(|_| Error::Invalid)?,
        ));
        if field::<String>(&row, "payload_hash")? != hash {
            return Err(Error::Unavailable);
        }
        let source = WebmailSource {
            scope_id: access.scope_id,
            grant_revision: access.grant_revision,
            subject: access.subject,
            mailbox_id: access.mailbox_id,
            write_id: input.operation_id.clone(),
            email_id: input.email_id,
            identity_id: input.identity_id,
            sender_name: access.sender_name,
        };
        let result = outbox::enqueue(
            &tx,
            &Enqueue {
                id: input.operation_id,
                customer_id: access.customer_id.clone(),
                source_id: format!("webmail:{}", source.write_id),
                idempotency_key: input.request_id.to_string(),
                request_hash: input.request_hash,
                payload_ref: input.payload_ref.clone(),
                payload_bytes: input.payload_bytes,
                route: serde_json::to_value(snapshot.route(&budget.path))
                    .map_err(|_| Error::Unavailable)?,
                envelope: serde_json::to_value(input.envelope).map_err(|_| Error::Invalid)?,
                payload_format: PayloadFormat::Mime,
                source_context: serde_json::to_value(source).map_err(|_| Error::Invalid)?,
                staging: true,
                not_before: None,
            },
        )
        .await?;
        run(
            &tx,
            "UPDATE webdock_mail.webmail_write SET queue_preparing_until=NULL WHERE id=$1",
            vec![result.id.clone().into()],
        )
        .await?;
        WebmailService::finish(tx, authorized).await?;
        Ok(
            json!({"id":result.id,"state":result.state,"created":result.created,"uploadToken":result.upload_token,"customerID":access.customer_id,"payloadRef":input.payload_ref}),
        )
    }
    pub(crate) async fn queued_access(
        &self,
        tx: &DatabaseTransaction,
        job: &SendLease,
    ) -> Result<QueuedAccess> {
        let source: WebmailSource =
            serde_json::from_value(job.source_context.clone()).map_err(|_| Error::Unavailable)?;
        if job.source_id != format!("webmail:{}", source.write_id)
            || job.id != source.write_id
            || !valid_id(&source.mailbox_id)
            || !valid_id(&source.grant_revision)
            || !object_id(&source.email_id)
            || !object_id(&source.identity_id)
        {
            return Err(Error::Unavailable);
        }
        let config = self.config.incoming.as_ref().ok_or(Error::Unavailable)?;
        one(tx,"SELECT id FROM webdock_mail.webmail_write WHERE id=$1 AND customer_id=$2 AND mailbox_id=$3 AND subject=$4 AND resource_id=$5 AND action='submit' AND state<>'rejected'",vec![source.write_id.into(),job.customer_id.clone().into(),source.mailbox_id.clone().into(),source.subject.clone().into(),source.email_id.into()]).await?.ok_or(Error::Unavailable)?;
        one(tx,"SELECT c.id FROM webdock_admin.customers c JOIN webdock_mail.service s ON s.customer_id=c.id WHERE c.id=$1 AND c.status='active' AND s.desired_enabled AND s.state='ready' AND s.revision=s.applied_revision AND s.host_id=$2 FOR SHARE OF c,s",vec![job.customer_id.clone().into(),config.managed_cluster.clone().into()]).await?.ok_or(Error::Unavailable)?;
        let user=one(tx,"SELECT role FROM webdock_auth.\"user\" WHERE id=$1 AND NOT coalesce(banned,false) AND \"emailVerified\" AND NOT \"mustChangePassword\" AND (role='user' OR (role='operator' AND \"twoFactorEnabled\")) FOR SHARE",vec![source.subject.clone().into()]).await?.ok_or(Error::Unavailable)?;
        let operator = field::<String>(&user, "role")? == "operator";
        let member=one(tx,"SELECT m.id FROM webdock_auth.member m JOIN webdock_auth.tenant_customer t ON t.organization_id=m.\"organizationId\" WHERE t.customer_id=$1 AND m.\"userId\"=$2 AND m.role<>'' FOR SHARE OF m",vec![job.customer_id.clone().into(),source.subject.clone().into()]).await?;
        let member = member.map(|row| field::<String>(&row, "id")).transpose()?;
        if !operator && member.is_none() {
            return Err(Error::Unavailable);
        }
        let row=one(tx,"SELECT b.name,b.provider_id AS account_id,d.name AS domain,b.local_part,i.id AS identity_id,i.encrypted_credentials FROM webdock_mail.mailbox b JOIN webdock_mail.domain d ON d.id=b.domain_id AND d.customer_id=b.customer_id JOIN webdock_mail.mailbox_grant g ON g.mailbox_id=b.id AND g.customer_id=b.customer_id AND g.subject=$3 JOIN webdock_mail.identity i ON i.customer_id=g.customer_id AND i.subject=g.subject WHERE b.id=$1 AND b.customer_id=$2 AND b.enabled AND g.enabled AND g.revision=g.applied_revision AND g.revision=$4::bigint AND b.state='ready' AND i.state='ready' AND b.provider_id IS NOT NULL AND i.provider_id IS NOT NULL AND g.clerk_grant IS NULL AND (g.member_id=$5 OR (g.operator_grant AND $6)) FOR SHARE OF b,g,i",vec![source.mailbox_id.clone().into(),job.customer_id.clone().into(),source.subject.clone().into(),source.grant_revision.into(),member.into(),operator.into()]).await?.ok_or(Error::Unavailable)?;
        #[derive(Deserialize)]
        struct Credential {
            username: String,
            password: String,
        }
        let identity: String = field(&row, "identity_id")?;
        let secret: Credential = crate::secrets::MailCipher::new(&config.encryption_key)?.open(
            &field::<Option<String>>(&row, "encrypted_credentials")?.ok_or(Error::Unavailable)?,
            &format!("identity:{}:{identity}", job.customer_id),
        )?;
        if secret.username.is_empty()
            || secret.username.contains([':', '\r', '\n', '\0'])
            || secret.password.is_empty()
            || secret.password.contains(['\r', '\n', '\0'])
        {
            return Err(Error::Unavailable);
        }
        Ok(QueuedAccess {
            customer_id: job.customer_id.clone(),
            mailbox_id: source.mailbox_id,
            account_id: field(&row, "account_id")?,
            sender_name: field(&row, "name")?,
            sender_address: format!(
                "{}@{}",
                field::<String>(&row, "local_part")?,
                field::<String>(&row, "domain")?
            ),
            runtime_origin: format!(
                "http://app-{}.wd-mail-{}.svc.cluster.local:8080",
                job.customer_id, job.customer_id
            ),
            authorization: format!(
                "Basic {}",
                STANDARD.encode(format!("{}:{}", secret.username, secret.password))
            ),
            subject: source.subject,
        })
    }
    async fn filing_access(
        &self,
        tx: DatabaseTransaction,
        service: &WebmailService,
        id: &str,
        token: Option<String>,
        customer: Option<String>,
        lease: Option<uuid::Uuid>,
    ) -> Result<Value> {
        if !valid_id(id) {
            return Err(Error::Invalid);
        }
        let row=one(&tx,"SELECT customer_id,lease_token::text,state,source_context,source_id,payload_ref::text,filing_state FROM webdock_mail.send_job WHERE id=$1 AND state='accepted' AND payload_format='mime'",vec![id.into()]).await?.ok_or(Error::Unavailable)?;
        let job = SendLease {
            id: id.into(),
            customer_id: field(&row, "customer_id")?,
            source_id: field(&row, "source_id")?,
            source_context: field(&row, "source_context")?,
            payload_ref: field(&row, "payload_ref")?,
            payload_bytes: 0,
            request_hash: String::new(),
            payload_format: PayloadFormat::Mime,
            route: Value::Null,
            envelope: Value::Null,
            lease_token: field(&row, "lease_token")?,
        };
        let source: WebmailSource =
            serde_json::from_value(job.source_context.clone()).map_err(|_| Error::Unavailable)?;
        let (access, authorized) = if let Some(token) = token {
            let (access, authorized) = service.inspect_in(&tx, &token).await?;
            if access.customer_id != job.customer_id || access.mailbox_id != source.mailbox_id {
                return Err(Error::Unavailable);
            }
            (
                serde_json::to_value(access).map_err(|_| Error::Unavailable)?,
                Some(authorized),
            )
        } else {
            if customer.as_deref() != Some(&job.customer_id)
                || lease.map(|v| v.to_string()).as_deref() != Some(&job.lease_token)
            {
                return Err(Error::Unavailable);
            }
            (
                serde_json::to_value(self.queued_access(&tx, &job).await?)
                    .map_err(|_| Error::Unavailable)?,
                None,
            )
        };
        let result = json!({"access":access,"emailID":source.email_id,"complete":field::<Option<String>>(&row,"filing_state")?.as_deref()==Some("complete"),"lease":{"id":job.id,"customerID":job.customer_id,"leaseToken":job.lease_token},"payloadRef":job.payload_ref});
        if let Some(authorized) = authorized {
            WebmailService::finish(tx, authorized).await?;
        } else {
            tx.commit().await.map_err(|_| Error::Unavailable)?;
        }
        Ok(result)
    }
}
fn status_row(row: &sea_orm::QueryResult) -> Result<Value> {
    let state = field::<Option<String>>(row, "state")?.unwrap_or_else(|| {
        match field::<String>(row, "write_state").as_deref() {
            Ok("rejected") => "rejected",
            Ok("reserved") => "staging",
            _ => "unknown",
        }
        .into()
    });
    let filing = field::<Option<String>>(row, "filing_state")?;
    Ok(
        json!({"id":field::<String>(row,"id")?,"state":state,"filed":filing.as_deref()==Some("complete"),"filingState":filing}),
    )
}
async fn status(
    tx: &DatabaseTransaction,
    access: &GatewayAccess,
    id: &str,
    cancel: bool,
) -> Result<Value> {
    if !valid_id(id) {
        return Err(Error::Invalid);
    }
    if cancel {
        crate::submissions::completion_lock(tx, &access.customer_id).await?;
    }
    let row=one(tx,"SELECT w.id,w.state AS write_state,j.state,j.filing_state,j.payload_ref::text FROM webdock_mail.webmail_write w LEFT JOIN webdock_mail.send_job j ON j.id=w.id AND j.customer_id=w.customer_id AND j.payload_format='mime' WHERE w.id=$1 AND w.customer_id=$2 AND w.mailbox_id=$3 AND w.action='submit' FOR UPDATE OF w",vec![id.into(),access.customer_id.clone().into(),access.mailbox_id.clone().into()]).await?.ok_or(Error::Unavailable)?;
    let mut result = status_row(&row)?;
    if cancel && outbox::cancel(tx, &access.customer_id, id).await? {
        budget::settle(tx, &access.customer_id, id, Settlement::Released).await?;
        crate::submissions::settle_journal(tx, id, "cancelled").await?;
        result["state"] = json!("cancelled");
        result["cleanupRef"] = json!(field::<String>(&row, "payload_ref")?);
    } else if cancel && field::<Option<String>>(&row, "state")?.is_none() && run(tx,"UPDATE webdock_mail.webmail_write SET state='rejected',queue_preparing_until=NULL WHERE id=$1 AND state='reserved' AND NOT EXISTS(SELECT 1 FROM webdock_mail.send_job WHERE id=$1)",vec![id.into()]).await? {result["state"]=json!("rejected");}
    Ok(result)
}
