//! Confidential Webmail export and filing, sharing the native encrypted payload queue.
use crate::{
    Error, Result,
    jmap::{JmapClient, MailAddress},
    message::{Sender, prepare_message},
    outbox::PayloadLease,
    payload::PrivatePayloadStore,
    proof::valid_id,
    queued_webmail::QueuedAccess,
    send_control::{ControlError, SendControl},
    webmail::GatewayAccess,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "lowercase", deny_unknown_fields)]
pub enum Command {
    Submit {
        token: String,
        #[serde(rename = "requestID")]
        request_id: uuid::Uuid,
        #[serde(rename = "emailID")]
        email_id: String,
        #[serde(rename = "identityID")]
        identity_id: String,
    },
    Lookup {
        token: String,
        #[serde(rename = "emailID")]
        email_id: String,
    },
    Status {
        token: String,
        id: String,
    },
    Cancel {
        token: String,
        id: String,
    },
    File {
        token: String,
        id: String,
    },
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Prepared {
    existing: bool,
    id: Option<String>,
    state: Option<String>,
    filed: Option<bool>,
    filing_state: Option<String>,
    #[serde(rename = "operationID")]
    operation_id: Option<String>,
    lease_token: Option<uuid::Uuid>,
    access: Option<GatewayAccess>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Staged {
    id: String,
    state: String,
    created: bool,
    upload_token: uuid::Uuid,
    #[serde(rename = "customerID")]
    customer_id: String,
    payload_ref: uuid::Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Committed {
    queued: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StatusReply {
    id: String,
    state: String,
    filed: bool,
    filing_state: Option<String>,
    cleanup_ref: Option<uuid::Uuid>,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum Access {
    Live(GatewayAccess),
    Queued(QueuedAccess),
}
impl Access {
    fn client(&self) -> Result<JmapClient> {
        let (customer, origin, authorization, account, name, address) = match self {
            Self::Live(a) => (
                &a.customer_id,
                &a.runtime_origin,
                &a.authorization,
                &a.account_id,
                &a.sender_name,
                &a.sender_address,
            ),
            Self::Queued(a) => (
                &a.customer_id,
                &a.runtime_origin,
                &a.authorization,
                &a.account_id,
                &a.sender_name,
                &a.sender_address,
            ),
        };
        if origin != &format!("http://app-{customer}.wd-mail-{customer}.svc.cluster.local:8080") {
            return Err(Error::Invalid);
        }
        JmapClient::assigned(
            customer,
            authorization,
            account,
            Some(MailAddress {
                email: address.clone(),
                name: Some(name.clone()),
            }),
        )
        .map_err(|_| Error::Unavailable)
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Filing {
    access: Access,
    #[serde(rename = "emailID")]
    email_id: String,
    complete: bool,
    lease: FilingLease,
    payload_ref: uuid::Uuid,
}
#[derive(Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FilingLease {
    id: String,
    #[serde(rename = "customerID")]
    customer_id: String,
    lease_token: uuid::Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Filed {
    recorded: bool,
    filed: bool,
}
async fn call<T: serde::de::DeserializeOwned>(
    control: &dyn SendControl,
    input: Value,
) -> std::result::Result<T, ControlError> {
    serde_json::from_value(control.call(input).await?).map_err(|_| ControlError::unavailable())
}
static BLOCKING: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
async fn blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T> + Send + 'static,
) -> std::result::Result<T, ControlError> {
    let permit = BLOCKING.acquire().await.map_err(unavailable)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        operation()
    })
    .await
    .map_err(unavailable)?
    .map_err(unavailable)
}
fn unavailable(_: impl std::fmt::Display) -> ControlError {
    ControlError::unavailable()
}
fn token(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn object(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
pub async fn execute(
    control: &dyn SendControl,
    store: Arc<PrivatePayloadStore>,
    command: Command,
) -> std::result::Result<Value, ControlError> {
    match command {
        Command::Submit {
            token: scope,
            request_id,
            email_id,
            identity_id,
        } => {
            if !token(&scope) || !object(&email_id) || !object(&identity_id) {
                return Err(ControlError::invalid());
            }
            let input = json!({"token":scope,"requestID":request_id,"emailID":email_id,"identityID":identity_id});
            let prepared: Prepared = call(
                control,
                json!({"action":"send.webmail.prepare","submission":input}),
            )
            .await?;
            if prepared.existing {
                let id = prepared
                    .id
                    .filter(|v| valid_id(v))
                    .ok_or_else(ControlError::unavailable)?;
                return Ok(
                    json!({"id":id,"state":prepared.state,"filed":prepared.filed==Some(true),"filingState":prepared.filing_state}),
                );
            }
            let operation = prepared
                .operation_id
                .filter(|v| valid_id(v))
                .ok_or_else(ControlError::unavailable)?;
            let lease = prepared.lease_token.ok_or_else(ControlError::unavailable)?;
            let result = export(
                control,
                store,
                input,
                prepared.access.ok_or_else(ControlError::unavailable)?,
                &operation,
                lease,
                (&email_id, &identity_id),
            )
            .await;
            if result.is_err() {
                let _=control.call(json!({"action":"send.webmail.abort","operationID":operation,"leaseToken":lease})).await;
            }
            result
        }
        Command::Lookup {
            token: scope,
            email_id,
        } => {
            if !token(&scope) || !object(&email_id) {
                return Err(ControlError::invalid());
            }
            let value: Option<StatusReply> = call(
                control,
                json!({"action":"send.webmail.lookup","token":scope,"emailID":email_id}),
            )
            .await?;
            Ok(value.map(|v|json!({"id":v.id,"state":v.state,"filed":v.filed,"filingState":v.filing_state})).unwrap_or(Value::Null))
        }
        Command::File { token: scope, id } => {
            if !token(&scope) || !valid_id(&id) {
                return Err(ControlError::invalid());
            }
            file(
                control,
                store,
                json!({"action":"send.webmail.filingAccess","token":scope,"id":id}),
            )
            .await
        }
        command @ (Command::Status { .. } | Command::Cancel { .. }) => {
            let (scope, id, cancel) = match command {
                Command::Status { token, id } => (token, id, false),
                Command::Cancel { token, id } => (token, id, true),
                _ => return Err(ControlError::invalid()),
            };
            if !token(&scope) || !valid_id(&id) {
                return Err(ControlError::invalid());
            }
            let reply: StatusReply = call(
                control,
                json!({"action":"send.webmail.status","token":scope,"id":id,"cancel":cancel}),
            )
            .await?;
            if reply.id != id {
                return Err(ControlError::unavailable());
            }
            if let Some(reference) = reply.cleanup_ref {
                blocking(move || store.remove(&reference.to_string())).await?;
            }
            Ok(
                json!({"id":reply.id,"state":reply.state,"filed":reply.filed,"filingState":reply.filing_state}),
            )
        }
    }
}
async fn export(
    control: &dyn SendControl,
    store: Arc<PrivatePayloadStore>,
    mut input: Value,
    access: GatewayAccess,
    operation: &str,
    lease: uuid::Uuid,
    draft_ids: (&str, &str),
) -> std::result::Result<Value, ControlError> {
    let customer = access.customer_id.clone();
    let sender = Sender {
        email: access.sender_address.clone(),
        name: Some(access.sender_name.clone()),
    };
    let draft = Access::Live(access)
        .client()
        .map_err(unavailable)?
        .export_draft(draft_ids.0, draft_ids.1)
        .await
        .map_err(unavailable)?;
    if draft.sender.email != sender.email {
        return Err(ControlError::unavailable());
    }
    let message = blocking(move || Ok(prepare_message(&draft.raw, &sender, &draft.to)?)).await?;
    let reference = uuid::Uuid::new_v4();
    input["operationID"] = json!(operation);
    input["leaseToken"] = json!(lease);
    input["payloadRef"] = json!(reference);
    input["payloadBytes"] = json!(message.raw.len());
    input["requestHash"] = json!(webdock_mail_protocol::sha256(&message.raw));
    input["envelope"] = json!({"from":message.from,"to":message.to});
    let staged: Staged = call(
        control,
        json!({"action":"send.webmail.stage","submission":input}),
    )
    .await?;
    if staged.id != operation
        || !staged.created
        || staged.state != "staging"
        || staged.customer_id != customer
        || staged.payload_ref != reference
    {
        return Err(ControlError::unavailable());
    }
    let upload = PayloadLease {
        id: staged.id.clone(),
        customer_id: customer.clone(),
        payload_ref: reference.to_string(),
        upload_token: staged.upload_token.to_string(),
    };
    let saved =
        blocking(move || store.put(&customer, &message.raw, Some(&reference.to_string()))).await;
    if saved.is_err() {
        let _ = control
            .call(json!({"action":"send.abortPayload","lease":upload}))
            .await;
        return Err(ControlError::unavailable());
    }
    let committed: Committed = call(
        control,
        json!({"action":"send.commitPayload","lease":upload}),
    )
    .await?;
    if !committed.queued {
        return Err(ControlError::unavailable());
    }
    Ok(json!({"id":staged.id,"state":"queued","filed":false}))
}
async fn file(
    control: &dyn SendControl,
    store: Arc<PrivatePayloadStore>,
    input: Value,
) -> std::result::Result<Value, ControlError> {
    let info: Filing = call(control, input).await?;
    if info.complete {
        return Ok(json!({"id":info.lease.id,"state":"accepted","filed":true}));
    }
    let filed = info
        .access
        .client()
        .map_err(unavailable)?
        .file_sent(&info.email_id)
        .await
        .is_ok();
    let result:Filed=call(control,json!({"action":"send.webmail.filed","id":info.lease.id,"customerID":info.lease.customer_id,"leaseToken":info.lease.lease_token,"filed":filed})).await?;
    if !result.recorded {
        return Err(ControlError::unavailable());
    }
    if result.filed {
        blocking(move || store.remove(&info.payload_ref.to_string())).await?;
    }
    Ok(json!({"id":info.lease.id,"state":"accepted","filed":result.filed}))
}
pub async fn file_queued(
    control: &dyn SendControl,
    store: Arc<PrivatePayloadStore>,
    lease: &crate::outbox::SendLease,
) -> std::result::Result<Value, ControlError> {
    file(control,store,json!({"action":"send.webmail.filingAccess","id":lease.id,"customerID":lease.customer_id,"leaseToken":lease.lease_token})).await
}
