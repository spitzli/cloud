//! The installed SMTP data hook authenticates before accepting its bounded MIME observation.
use crate::outbox::{field, one, run};
use crate::{
    Error, Result,
    sending::{Envelope, Sending},
};
use serde::Deserialize;
use serde_json::{Value, json};
use spitzli_mail_protocol::{sha256, valid_digest};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PolicySubmission {
    #[serde(rename = "customerID")]
    pub customer_id: String,
    pub token: String,
    pub id: String,
    pub proof: String,
    pub digest: String,
    pub username: String,
    #[serde(rename = "queueID")]
    pub queue_id: String,
    pub envelope: Envelope,
}
pub(crate) async fn authenticate(
    tx: &sea_orm::DatabaseTransaction,
    customer: &str,
    token: &str,
) -> Result<()> {
    let suffix = token
        .strip_prefix(&format!("wd_hook_{customer}_"))
        .ok_or(Error::Invalid)?;
    if !crate::proof::valid_id(customer)
        || suffix.len() != 43
        || !suffix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(Error::Invalid);
    }
    one(tx,"SELECT i.customer_id FROM webdock_mail.instance i JOIN webdock_mail.service s USING(customer_id) JOIN webdock_admin.customers c ON c.id=i.customer_id WHERE i.customer_id=$1 AND i.submission_hook_token_hash=$2 AND s.desired_enabled AND c.status='active' FOR SHARE OF i,s,c",vec![customer.into(),sha256(token.as_bytes()).into()]).await?.ok_or(Error::Unavailable)?;
    Ok(())
}
impl Sending {
    pub(crate) async fn policy(
        &self,
        tx: &sea_orm::DatabaseTransaction,
        input: PolicySubmission,
    ) -> Result<Value> {
        input.envelope.validate()?;
        if !crate::proof::valid_id(&input.id)
            || !valid_digest(&input.digest)
            || input.username.len() > 320
            || input.queue_id.is_empty()
            || input.queue_id.len() > 32
            || !input
                .queue_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Invalid);
        }
        // Acquire provider/customer locks before locking the admitted job and gate receipt.
        let snapshot = self
            .policy
            .snapshot(tx, &input.customer_id, true, None)
            .await?;
        authenticate(tx, &input.customer_id, &input.token).await?;
        let row=one(tx,"SELECT lease_token::text,state FROM webdock_mail.send_job WHERE id=$1 AND customer_id=$2 AND state IN ('handoff','unknown')",vec![input.id.clone().into(),input.customer_id.clone().into()]).await?.ok_or(Error::Unavailable)?;
        let token: String = field(&row, "lease_token")?;
        let state: String = field(&row, "state")?;
        let job = super::sending::read_job(tx, &input.id, &input.customer_id, &token, &state)
            .await?
            .ok_or(Error::Unavailable)?;
        let path = self.current(tx, &job, &snapshot).await?;
        let crate::budget::SendPath::Stalwart { service_revision } = path else {
            return Err(Error::Unavailable);
        };
        let envelope: Envelope =
            serde_json::from_value(job.envelope.clone()).map_err(|_| Error::Unavailable)?;
        fn recipients(values: &[String]) -> std::collections::BTreeSet<&String> {
            values.iter().collect()
        }
        if envelope.from != input.envelope.from
            || recipients(&envelope.to) != recipients(&input.envelope.to)
        {
            return Err(Error::Unavailable);
        }
        let config = self.config.incoming.as_ref().ok_or(Error::Unavailable)?;
        let connection = config
            .connection(
                tx,
                &job.customer_id,
                &service_revision,
                snapshot.transport_revision,
            )
            .await?;
        if input.username != connection.username
            || !crate::proof::verify_submission(
                &crate::proof::SubmissionProof {
                    customer_id: job.customer_id.clone(),
                    id: job.id.clone(),
                    digest: input.digest.clone(),
                    from: envelope.from,
                    to: envelope.to,
                    transport_revision: snapshot.transport_revision.to_string(),
                    service_revision,
                },
                &input.proof,
                &config.encryption_key,
            )?
        {
            return Err(Error::Unavailable);
        }
        super::sending::lock_job(tx, &job, &state).await?;
        let row = one(
            tx,
            "SELECT message_digest,submission_gate_queue_id FROM webdock_mail.send_job WHERE id=$1",
            vec![job.id.clone().into()],
        )
        .await?
        .ok_or(Error::Unavailable)?;
        if field::<Option<String>>(&row, "message_digest")?.as_deref() != Some(&input.digest)
            || field::<Option<String>>(&row, "submission_gate_queue_id")?
                .is_some_and(|queue| queue != input.queue_id)
        {
            return Err(Error::Unavailable);
        }
        run(
            tx,
            "UPDATE webdock_mail.send_job SET submission_gate_queue_id=$2 WHERE id=$1",
            vec![job.id.into(), input.queue_id.into()],
        )
        .await?;
        Ok(json!({"accept":true}))
    }
}
#[derive(Deserialize)]
struct Hook {
    context: Context,
    envelope: HookEnvelope,
    message: Message,
}
#[derive(Deserialize)]
struct Context {
    stage: String,
    sasl: Sasl,
    queue: Queue,
}
#[derive(Deserialize)]
struct Sasl {
    login: String,
}
#[derive(Deserialize)]
struct Queue {
    id: String,
}
#[derive(Deserialize)]
struct HookEnvelope {
    from: Address,
    to: Vec<Address>,
}
#[derive(Deserialize)]
struct Address {
    address: String,
}
#[derive(Deserialize)]
struct Message {
    headers: Vec<(String, String)>,
    contents: String,
    size: usize,
}
pub(crate) async fn handle(
    control: &dyn crate::send_control::SendControl,
    customer: &str,
    request: axum::extract::Request,
) -> axum::response::Response {
    use axum::{Json, http::StatusCode, response::IntoResponse};
    let fail = |status| {
        (
            status,
            [
                ("cache-control", "no-store"),
                ("x-content-type-options", "nosniff"),
            ],
            Json(json!({})),
        )
            .into_response()
    };
    let mut auth = request.headers().get_all("authorization").iter();
    let token = auth
        .next()
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if auth.next().is_some() {
        return fail(StatusCode::UNAUTHORIZED);
    }
    let Some(token) = token
        .filter(|v| v.starts_with(&format!("wd_hook_{customer}_")))
        .map(str::to_owned)
    else {
        return fail(StatusCode::UNAUTHORIZED);
    };
    let result:std::result::Result<Value,crate::send_control::ControlError>=async {
        let auth=control.call(json!({"action":"send.policy.auth","customerID":customer,"token":token})).await?;
        if auth["ok"]!=true {return Err(crate::send_control::ControlError::unavailable());}
        let raw=tokio::time::timeout(std::time::Duration::from_secs(20),axum::body::to_bytes(request.into_body(),52*1024*1024)).await.map_err(|_|crate::send_control::ControlError::unavailable())?.map_err(|_|crate::send_control::ControlError::invalid())?;
        let hook:Hook=serde_json::from_slice(&raw).map_err(|_|crate::send_control::ControlError::invalid())?;
        if hook.context.stage!="data" {return Err(crate::send_control::ControlError::invalid());}
        let proofs=hook.message.headers.iter().filter(|(name,_)|name.eq_ignore_ascii_case("x-webdock-submission")).collect::<Vec<_>>();
        let rejected=json!({"action":"reject","response":{"status":550,"enhanced_status":"5.7.1","message":"Spitzli Cloud submission is not authorized."}});
        if proofs.len()!=1 {return Ok(rejected);}
        let Some((id,proof))=proofs[0].1.trim().split_once('.') else {return Ok(rejected);};
        let digest=crate::proof::hook_digest(&hook.message.headers,&hook.message.contents,hook.message.size).map_err(|_|crate::send_control::ControlError::invalid())?;
        let result=control.call(json!({"action":"send.policy","submission":{"customerID":customer,"token":token,"id":id,"proof":proof,"digest":digest,"username":hook.context.sasl.login,"queueID":hook.context.queue.id,"envelope":{"from":hook.envelope.from.address,"to":hook.envelope.to.into_iter().map(|v|v.address).collect::<Vec<_>>()}}})).await?;
        if result["accept"]!=true {return Ok(rejected);}
        Ok(json!({"action":"accept","modifications":[{"type":"deleteHeader","index":1,"name":"X-Webdock-Submission"}]}))
    }.await;
    match result {
        Ok(value) => (
            StatusCode::OK,
            [
                ("cache-control", "no-store"),
                ("x-content-type-options", "nosniff"),
            ],
            Json(value),
        )
            .into_response(),
        Err(_) => fail(StatusCode::SERVICE_UNAVAILABLE),
    }
}
