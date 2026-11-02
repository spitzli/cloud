//! Public sending ingress. Authenticate before reading payloads, persist privately before enqueue
//! acknowledgement, and retain uncertain staging records for explicit reconciliation.
use crate::{
    PrivatePayloadStore,
    message::SendRequest,
    proof::valid_id,
    send_control::{ControlError, SendControl},
};
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Semaphore;
use uuid::Uuid;

pub struct SendApi {
    control: Arc<dyn SendControl>,
    store: Arc<dyn PrivatePayloadStore>,
    slots: Arc<Semaphore>,
    customers: Mutex<HashSet<String>>,
}
struct CustomerGuard {
    api: Arc<SendApi>,
    customer: String,
}
impl Drop for CustomerGuard {
    fn drop(&mut self) {
        if let Ok(mut customers) = self.api.customers.lock() {
            customers.remove(&self.customer);
        }
    }
}
fn fail(status: u16) -> ControlError {
    ControlError { status }
}
fn single<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    values.next().is_none().then_some(value)
}
pub(crate) fn token(headers: &HeaderMap) -> Result<&str, ControlError> {
    let value = single(headers, "authorization")
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or_else(|| fail(401))?;
    let suffix = value.strip_prefix("wd_mail_").ok_or_else(|| fail(401))?;
    if suffix.len() != 43
        || !suffix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(fail(401));
    }
    Ok(value)
}
fn state(value: &Value) -> Result<&str, ControlError> {
    value
        .as_str()
        .filter(|s| {
            matches!(
                *s,
                "staging"
                    | "queued"
                    | "leased"
                    | "preparation_review"
                    | "handoff"
                    | "unknown"
                    | "accepted"
                    | "rejected"
                    | "cancelled"
            )
        })
        .ok_or_else(ControlError::unavailable)
}
fn uuid(value: &Value) -> Option<&str> {
    let s = value.as_str()?;
    Uuid::parse_str(s)
        .ok()
        .filter(|u| {
            u.get_version_num() == 4
                && u.get_variant() == uuid::Variant::RFC4122
                && u.to_string() == s
        })
        .map(|_| s)
}
pub(crate) fn reply(status: StatusCode, body: Value) -> Response {
    let mut response = (status, Json(body)).into_response();
    let headers = response.headers_mut();
    headers.insert("cache-control", "no-store".parse().unwrap());
    headers.insert("x-content-type-options", "nosniff".parse().unwrap());
    if status.is_client_error() || status.is_server_error() {
        headers.insert("connection", "close".parse().unwrap());
    }
    response
}
impl SendApi {
    pub fn new(control: Arc<dyn SendControl>, store: Arc<dyn PrivatePayloadStore>) -> Arc<Self> {
        Arc::new(Self {
            control,
            store,
            slots: Arc::new(Semaphore::new(4)),
            customers: Mutex::new(HashSet::new()),
        })
    }
    /// Shared with compatibility internal routes so the combined listener retains four slots.
    pub fn ingress_slots(&self) -> Arc<Semaphore> {
        self.slots.clone()
    }
    pub async fn handle(self: Arc<Self>, request: Request) -> Response {
        handle(State(self), request).await
    }
    pub fn router(self: Arc<Self>) -> Router {
        Router::new().fallback(any(handle)).with_state(self)
    }
}
async fn handle(State(api): State<Arc<SendApi>>, request: Request) -> Response {
    if request.method() == Method::GET
        && matches!(
            request.uri().path_and_query().map(|p| p.as_str()),
            Some("/health" | "/healthz")
        )
    {
        return reply(StatusCode::OK, json!({"ok":true}));
    }
    let Ok(permit) = api.slots.clone().try_acquire_owned() else {
        return reply(
            StatusCode::TOO_MANY_REQUESTS,
            json!({"error":"Mail is busy. Try later."}),
        );
    };
    let permit = Arc::new(permit);
    let mut submission = None;
    let result:Result<(StatusCode,Value),ControlError>=async{
        let token=token(request.headers())?.to_owned();
        let path=request.uri().path_and_query().map(|p|p.as_str()).unwrap_or_default();
        if let Some(id)=path.strip_prefix("/v1/emails/").filter(|id|valid_id(id))
            && matches!(*request.method(),Method::GET|Method::DELETE){
            let result=api.control.call(json!({"action":"send.status","token":token,"id":id,"cancel":request.method()==Method::DELETE})).await?;
            if result["id"]!=id{return Err(ControlError::unavailable());}let status=state(&result["state"])?;
            if let Some(reference)=result.get("cleanupRef"){
                let reference=uuid(reference).ok_or_else(ControlError::unavailable)?.to_owned();
                // Legacy status replies contain only a reference. Resolve its tenant through the same key before scoped cleanup.
                let access=api.control.call(json!({"action":"send.authenticate","token":token})).await?;
                let customer=access["customerID"].as_str().filter(|s|valid_id(s)).ok_or_else(ControlError::unavailable)?;
                api.store.remove(customer,&reference).await.map_err(|_|ControlError::unavailable())?;
            }
            return Ok((StatusCode::OK,json!({"id":id,"state":status})));
        }
        if path!="/v1/emails"||request.method()!=Method::POST{return Err(fail(404));}
        let idempotency=if request.headers().contains_key("idempotency-key"){
            single(request.headers(),"idempotency-key").filter(|s|!s.is_empty()&&s.len()<=200&&s.bytes().all(|b|(0x21..=0x7e).contains(&b))).ok_or_else(||fail(400))?.to_owned()
        }else{Uuid::new_v4().to_string()};
        let access=api.control.call(json!({"action":"send.authenticate","token":token})).await?;
        let customer=access["customerID"].as_str().filter(|s|valid_id(s)).ok_or_else(ControlError::unavailable)?.to_owned();
        {let mut customers=api.customers.lock().map_err(|_|ControlError::unavailable())?;if !customers.insert(customer.clone()){return Err(fail(429));}}
        let customer_guard=Arc::new(CustomerGuard{api:api.clone(),customer:customer.clone()});
        if !single(request.headers(),"content-type").is_some_and(|s|s.split(';').next().is_some_and(|s|s.eq_ignore_ascii_case("application/json"))){return Err(fail(415));}
        let body=read(request.into_body()).await?;
        let slot=permit.clone();let guard=customer_guard.clone();
        let (payload,envelope,scheduled)=tokio::task::spawn_blocking(move||{
            let (_slot,_guard)=(slot,guard);
            let request=SendRequest::parse(serde_json::from_slice(&body).map_err(|_|fail(400))?,chrono::Utc::now()).map_err(|_|fail(400))?;
            request.compile().map_err(|_|fail(413))?;
            let envelope=json!({"from":request.from.email,"to":request.to.iter().chain(&request.cc).chain(&request.bcc).collect::<Vec<_>>()});
            let payload=serde_json::to_vec(&request).map_err(|_|fail(400))?;
            Ok::<_,ControlError>((payload,envelope,request.scheduled_at))
        }).await.map_err(|_|ControlError::unavailable())??;
        let reference=Uuid::new_v4().to_string();
        let mut admission=json!({"token":token,"idempotencyKey":idempotency,"requestHash":hex::encode(Sha256::digest(&payload)),"payloadRef":reference,"payloadBytes":payload.len(),"envelope":envelope});
        if let Some(at)=scheduled{admission["notBefore"]=Value::String(at);}
        let result=api.control.call(json!({"action":"send.admit","submission":admission})).await?;
        let id=result["id"].as_str().filter(|s|valid_id(s)).ok_or_else(ControlError::unavailable)?.to_owned();
        let created=result["created"].as_bool().ok_or_else(ControlError::unavailable)?;let status=state(&result["state"])?;
        submission=Some(id.clone());
        if !created{return Ok((if status=="staging"{StatusCode::CONFLICT}else{StatusCode::ACCEPTED},json!({"id":id,"state":status})));}
        if status!="staging"||result["customerID"]!=customer||result["payloadRef"]!=reference||uuid(&result["uploadToken"]).is_none(){return Err(ControlError::unavailable());}
        let lease=json!({"id":id,"customerID":customer,"payloadRef":reference,"uploadToken":result["uploadToken"]});
        let store=api.store.clone();let slot=permit.clone();let guard=customer_guard.clone();
        // Custody can outlive a disconnected request; keep exclusion until the write completes.
        let saved=tokio::spawn(async move { let (_slot,_guard)=(slot,guard);store.put(&customer,&reference,&payload).await }).await;
        if !matches!(saved,Ok(Ok(_))){let _=api.control.call(json!({"action":"send.abortPayload","lease":lease})).await;return Err(ControlError::unavailable());}
        let committed=api.control.call(json!({"action":"send.commitPayload","lease":lease})).await?;
        if committed["queued"]!=true{return Err(ControlError::unavailable());}
        Ok((StatusCode::ACCEPTED,json!({"id":id,"state":"queued"})))
    }.await;
    match result {
        Ok((status, body)) => reply(status, body),
        Err(error) => {
            let mut body = json!({"error":"Mail request could not be completed."});
            if let Some(id) = submission {
                body["id"] = Value::String(id);
            }
            reply(
                StatusCode::from_u16(error.status)
                    .ok()
                    .filter(|s| s.is_client_error() || s.is_server_error())
                    .unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
                body,
            )
        }
    }
}
async fn read(body: Body) -> Result<axum::body::Bytes, ControlError> {
    tokio::time::timeout(Duration::from_secs(20), to_bytes(body, 16 * 1024 * 1024))
        .await
        .map_err(|_| fail(408))?
        .map_err(|_| fail(413))
}
