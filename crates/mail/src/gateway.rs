//! Bounded browser gateway. Runtime credentials remain in this process; every operation rechecks
//! its native session, and the durable write journal is consulted before changing the Mail engine.
use crate::{
    jmap::{
        EmailChange, EmailQuery, JmapClient, JmapError, MAX_ATTACHMENT_BYTES, MailAddress,
        MailAttachment, MailDraft,
    },
    proof::valid_id,
};
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Semaphore;
const COOKIE: &str = "__Secure-webdock_mail";
#[derive(Debug, thiserror::Error)]
#[error("Mail control request is unavailable")]
pub struct ControlFailure;
#[async_trait::async_trait]
pub trait Control: Send + Sync {
    async fn call(&self, input: Value) -> std::result::Result<Value, ControlFailure>;
    /// Native queue only. The production gateway never calls JMAP EmailSubmission/set.
    async fn send(&self, input: Value) -> std::result::Result<Value, ControlFailure>;
}
pub struct GatewayOptions {
    origins: HashSet<String>,
    control: Arc<dyn Control>,
    /// Owned loopback transport only; Core must still attest the canonical assigned hostname.
    #[cfg(feature = "test-support")]
    pub test_runtime_origin: Option<String>,
}
impl GatewayOptions {
    pub fn new(
        origins: Vec<String>,
        control: Arc<dyn Control>,
    ) -> std::result::Result<Self, ControlFailure> {
        if origins.is_empty() || origins.len() > 32 {
            return Err(ControlFailure);
        }
        for origin in &origins {
            let url = url::Url::parse(origin).map_err(|_| ControlFailure)?;
            if url.scheme() != "https" || url.origin().ascii_serialization() != *origin {
                return Err(ControlFailure);
            }
        }
        Ok(Self {
            origins: origins.into_iter().collect(),
            control,
            #[cfg(feature = "test-support")]
            test_runtime_origin: None,
        })
    }
}
struct Session {
    token: String,
    scope: String,
    customer: String,
    mailbox: String,
    expires: DateTime<Utc>,
    active: Arc<Semaphore>,
    uploads: Mutex<Uploads>,
}
#[derive(Default)]
struct Uploads {
    bytes: usize,
    pending: usize,
    attachments: HashMap<String, MailAttachment>,
}
struct UploadSlot(Arc<Session>);
impl Drop for UploadSlot {
    fn drop(&mut self) {
        if let Ok(mut uploads) = self.0.uploads.lock() {
            uploads.pending = uploads.pending.saturating_sub(1);
        }
    }
}
pub struct Gateway {
    options: GatewayOptions,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    active: Arc<Semaphore>,
}
type Result<T> = std::result::Result<T, Failure>;
struct Failure {
    status: StatusCode,
    message: &'static str,
    uncertain: bool,
}
impl Failure {
    fn bad() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: "Invalid Mail request.",
            uncertain: false,
        }
    }
    fn denied() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: "Mail access expired or was revoked.",
            uncertain: false,
        }
    }
    fn unavailable() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: "Mail request is unavailable.",
            uncertain: false,
        }
    }
    fn busy() -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: "Mail is busy. Try again shortly.",
            uncertain: false,
        }
    }
    fn too_large() -> Self {
        Self {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            message: "Mail attachment or request is too large.",
            uncertain: false,
        }
    }
    fn unknown() -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            message: "Mail change outcome is unknown. Check its status before trying again.",
            uncertain: true,
        }
    }
}
impl From<JmapError> for Failure {
    fn from(e: JmapError) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            message: "Mail engine request failed.",
            uncertain: e.uncertain,
        }
    }
}
impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        let mut body = json!({"error":self.message});
        if self.uncertain {
            body["uncertain"] = json!(true);
        }
        (self.status, Json(body)).into_response()
    }
}
fn text(value: &Value, max: usize) -> Result<&str> {
    value
        .as_str()
        .filter(|s| !s.is_empty() && s.encode_utf16().count() <= max)
        .ok_or_else(Failure::bad)
}
fn record_id(value: &Value) -> Result<&str> {
    let id = text(value, 19)?;
    if !valid_id(id) {
        return Err(Failure::bad());
    }
    Ok(id)
}
fn fields(value: &Value, names: &[&str]) -> Result<()> {
    if !value
        .as_object()
        .is_some_and(|o| o.keys().all(|k| names.contains(&k.as_str())))
    {
        return Err(Failure::bad());
    }
    Ok(())
}
fn expires(value: &Value) -> Result<DateTime<Utc>> {
    let time = DateTime::parse_from_rfc3339(text(value, 64)?)
        .map_err(|_| Failure::denied())?
        .with_timezone(&Utc);
    if time <= Utc::now() || time > Utc::now() + chrono::Duration::minutes(16) {
        return Err(Failure::denied());
    }
    Ok(time)
}
fn token(value: &Value) -> Result<&str> {
    let value = text(value, 43)?;
    if value.len() != 43
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(Failure::bad());
    }
    Ok(value)
}
fn random() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|_| Failure::unavailable())?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
fn request_id(value: &Value) -> Result<String> {
    let id = uuid::Uuid::parse_str(text(value, 36)?).map_err(|_| Failure::bad())?;
    if id.get_version_num() != 4 || id.get_variant() != uuid::Variant::RFC4122 {
        return Err(Failure::bad());
    }
    Ok(id.to_string())
}
fn cookie(headers: &HeaderMap) -> Result<String> {
    let mut found = None;
    for header in headers.get_all(header::COOKIE) {
        for piece in header.to_str().map_err(|_| Failure::denied())?.split(';') {
            if let Some((name, value)) = piece.trim().split_once('=')
                && name == COOKIE
            {
                if found.is_some() {
                    return Err(Failure::denied());
                }
                found = Some(value.to_owned());
            }
        }
    }
    found.ok_or_else(Failure::denied)
}
async fn read_json(body: Body) -> Result<Value> {
    let bytes = tokio::time::timeout(Duration::from_secs(15), to_bytes(body, 524288))
        .await
        .map_err(|_| Failure::bad())?
        .map_err(|_| Failure::too_large())?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| Failure::bad())?;
    if !value.is_object() {
        return Err(Failure::bad());
    }
    Ok(value)
}
fn native_receipt(value: Value) -> Result<Value> {
    let id = record_id(&value["id"]).map_err(|_| Failure::unknown())?;
    let state = text(&value["state"], 32).map_err(|_| Failure::unknown())?;
    if ![
        "staging",
        "queued",
        "leased",
        "handoff",
        "accepted",
        "rejected",
        "cancelled",
        "unknown",
        "preparation_review",
    ]
    .contains(&state)
    {
        return Err(Failure::unknown());
    }
    let mut result = json!({"id":id,"state":state,"filed":value["filed"]==true});
    if let Some(filing) = value["filingState"]
        .as_str()
        .filter(|s| ["pending", "complete", "needs_review"].contains(s))
    {
        result["filingState"] = json!(filing);
    }
    Ok(result)
}
impl Gateway {
    pub fn new(options: GatewayOptions) -> std::result::Result<Self, ControlFailure> {
        #[cfg(feature = "test-support")]
        if let Some(origin) = &options.test_runtime_origin {
            let url = url::Url::parse(origin).map_err(|_| ControlFailure)?;
            if url.scheme() != "http"
                || url.host_str() != Some("127.0.0.1")
                || url.origin().ascii_serialization() != *origin
            {
                return Err(ControlFailure);
            }
        }
        Ok(Self {
            options,
            sessions: Mutex::new(HashMap::new()),
            active: Arc::new(Semaphore::new(16)),
        })
    }
    pub fn router(self) -> Router {
        Router::new().fallback(handle).with_state(Arc::new(self))
    }
    async fn inspect(&self, session: &Session) -> Result<Value> {
        if session.expires <= Utc::now() {
            return Err(Failure::denied());
        }
        let access = self
            .options
            .control
            .call(json!({"action":"inspect","token":session.token}))
            .await
            .map_err(|_| Failure::denied())?;
        if access["scopeID"] != session.scope
            || access["customerID"] != session.customer
            || access["mailboxID"] != session.mailbox
        {
            return Err(Failure::denied());
        }
        expires(&access["expiresAt"])?;
        let expected = format!(
            "http://app-{}.wd-mail-{}.svc.cluster.local:8080",
            session.customer, session.customer
        );
        if text(&access["runtimeOrigin"], 300)? != expected {
            return Err(Failure::denied());
        }
        Ok(access)
    }
    fn client(&self, session: &Session, access: &Value) -> Result<JmapClient> {
        let authorization = text(&access["authorization"], 4096)?;
        let account = text(&access["accountID"], 255)?;
        let sender = Some(MailAddress {
            email: text(&access["senderAddress"], 320)?.into(),
            name: Some(text(&access["senderName"], 80)?.into()),
        });
        #[cfg(feature = "test-support")]
        if let Some(origin) = &self.options.test_runtime_origin {
            return Ok(JmapClient::local_fixture(
                &format!("{origin}/"),
                authorization,
                account,
                sender,
            )?);
        }
        Ok(JmapClient::assigned(
            &session.customer,
            authorization,
            account,
            sender,
        )?)
    }
    async fn write<F, Fut>(
        &self,
        session: &Session,
        request: &Value,
        action: &str,
        payload: &Value,
        resource: Option<&str>,
        perform: F,
    ) -> Result<Value>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Value>>,
    {
        let request_id = request_id(&request["requestID"])?;
        let mut command = json!({"token":session.token,"requestID":request_id,"action":action,"payloadHash":hex::encode(Sha256::digest(serde_json::to_vec(payload).map_err(|_|Failure::bad())?))});
        if let Some(resource) = resource {
            command["resourceID"] = json!(resource);
        }
        let reservation = self
            .options
            .control
            .call(json!({"action":"reserve","write":command}))
            .await
            .map_err(|_| Failure::denied())?;
        if reservation["execute"] != true {
            if reservation["state"] == "succeeded" {
                return Ok(reservation["result"].clone());
            }
            return Err(Failure {
                status: StatusCode::CONFLICT,
                message: "Mail change is already recorded or its outcome is unknown.",
                uncertain: reservation["state"] != "rejected",
            });
        }
        let operation = record_id(&reservation["operationID"])?;
        let lease = uuid::Uuid::parse_str(text(&reservation["leaseToken"], 36)?)
            .map_err(|_| Failure::unknown())?;
        let result = perform().await;
        match result {
            Err(error) => {
                let _=self.options.control.call(json!({"action":"finish","write":{"token":session.token,"operationID":operation,"leaseToken":lease,"state":if error.uncertain{"uncertain"}else{"rejected"}}})).await;
                Err(error)
            }
            Ok(value) => {
                self.options.control.call(json!({"action":"finish","write":{"token":session.token,"operationID":operation,"leaseToken":lease,"state":"succeeded","result":value}})).await.map_err(|_|Failure::unknown())?;
                Ok(value)
            }
        }
    }
    async fn route(&self, request: Request) -> Result<Response> {
        let _global = self
            .active
            .clone()
            .try_acquire_owned()
            .map_err(|_| Failure::busy())?;
        if request.uri().query().is_some() {
            return Err(Failure::bad());
        }
        let path = request.uri().path().to_owned();
        let headers = request.headers().clone();
        let upload = path.ends_with("/upload");
        if !upload
            && (headers.get_all(header::CONTENT_TYPE).iter().count() != 1
                || headers
                    .get(header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.split(';').next())
                    .map(str::trim)
                    != Some("application/json"))
        {
            return Err(Failure {
                status: StatusCode::UNSUPPORTED_MEDIA_TYPE,
                message: "Use application/json.",
                uncertain: false,
            });
        }
        let body = request.into_body();
        if path == "/session" {
            let body = read_json(body).await?;
            fields(&body, &["code"])?;
            let code = token(&body["code"])?;
            {
                let mut sessions = self.sessions.lock().map_err(|_| Failure::unavailable())?;
                sessions.retain(|_, s| s.expires > Utc::now());
                if sessions.len() >= 4096 {
                    return Err(Failure::busy());
                }
            }
            let launch = self
                .options
                .control
                .call(json!({"action":"exchange","code":code}))
                .await
                .map_err(|_| Failure::denied())?;
            let session = Arc::new(Session {
                token: token(&launch["token"])?.into(),
                scope: record_id(&launch["scopeID"])?.into(),
                customer: record_id(&launch["customerID"])?.into(),
                mailbox: record_id(&launch["mailboxID"])?.into(),
                expires: expires(&launch["expiresAt"])?,
                active: Arc::new(Semaphore::new(4)),
                uploads: Mutex::new(Uploads::default()),
            });
            let cookie = random()?;
            let response = json!({"scopeID":session.scope,"customerID":session.customer,"mailboxID":session.mailbox,"expiresAt":session.expires});
            let mut response = Json(response).into_response();
            response.headers_mut().insert(
                header::SET_COOKIE,
                HeaderValue::from_str(&format!(
                    "{COOKIE}={cookie}; Path=/s/{}/; HttpOnly; Secure; SameSite=Strict; Max-Age={}",
                    session.scope,
                    (session.expires - Utc::now()).num_seconds().max(0)
                ))
                .map_err(|_| Failure::unavailable())?,
            );
            let mut sessions = self.sessions.lock().map_err(|_| Failure::unavailable())?;
            if sessions.len() >= 4096 {
                return Err(Failure::busy());
            }
            sessions.insert(cookie, session);
            return Ok(response);
        }
        let parts = path.split('/').collect::<Vec<_>>();
        if parts.len() != 4
            || parts[1] != "s"
            || !valid_id(parts[2])
            || !["mail", "logout", "upload", "download"].contains(&parts[3])
        {
            return Err(Failure::denied());
        }
        let cookie = cookie(&headers)?;
        let session = self
            .sessions
            .lock()
            .map_err(|_| Failure::unavailable())?
            .get(&cookie)
            .cloned()
            .ok_or_else(Failure::denied)?;
        if session.scope != parts[2] || session.expires <= Utc::now() {
            return Err(Failure::denied());
        }
        let _session = session
            .active
            .clone()
            .try_acquire_owned()
            .map_err(|_| Failure::busy())?;
        if parts[3] == "logout" {
            let body = read_json(body).await?;
            fields(&body, &[])?;
            self.sessions
                .lock()
                .map_err(|_| Failure::unavailable())?
                .remove(&cookie);
            self.options
                .control
                .call(json!({"action":"logout","token":session.token}))
                .await
                .map_err(|_| Failure::unavailable())?;
            let mut response = Json(json!({"ok":true})).into_response();
            response.headers_mut().insert(
                header::SET_COOKIE,
                HeaderValue::from_str(&format!(
                    "{COOKIE}=; Path=/s/{}/; HttpOnly; Secure; SameSite=Strict; Max-Age=0",
                    session.scope
                ))
                .map_err(|_| Failure::unavailable())?,
            );
            return Ok(response);
        }
        let access = self.inspect(&session).await?;
        let client = self.client(&session, &access)?;
        if upload {
            let slot = {
                let mut uploads = session.uploads.lock().map_err(|_| Failure::unavailable())?;
                if uploads.attachments.len() + uploads.pending >= 20
                    || uploads.bytes >= 2 * MAX_ATTACHMENT_BYTES
                {
                    return Err(Failure::too_large());
                }
                uploads.pending += 1;
                UploadSlot(session.clone())
            };
            let filename = headers
                .get("x-webdock-filename")
                .and_then(|v| v.to_str().ok())
                .filter(|s| s.len() <= 600)
                .ok_or_else(Failure::bad)?;
            let filename = percent_encoding::percent_decode_str(filename)
                .decode_utf8()
                .map_err(|_| Failure::bad())?
                .into_owned();
            if filename.is_empty()
                || filename.encode_utf16().count() > 200
                || filename.contains(['\r', '\n', '\0', '/', '\\'])
            {
                return Err(Failure::bad());
            }
            let media_type = headers
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(Failure::bad)?;
            let bytes = tokio::time::timeout(
                Duration::from_secs(15),
                to_bytes(body, MAX_ATTACHMENT_BYTES),
            )
            .await
            .map_err(|_| Failure::bad())?
            .map_err(|_| Failure::too_large())?;
            {
                let mut uploads = session.uploads.lock().map_err(|_| Failure::unavailable())?;
                if uploads.bytes + bytes.len() > 2 * MAX_ATTACHMENT_BYTES {
                    return Err(Failure::too_large());
                }
                uploads.bytes += bytes.len();
            }
            let mut attachment = client.upload(bytes.to_vec(), media_type).await?;
            attachment["name"] = json!(filename);
            let record: MailAttachment =
                serde_json::from_value(attachment.clone()).map_err(|_| Failure::unknown())?;
            self.inspect(&session).await?;
            session
                .uploads
                .lock()
                .map_err(|_| Failure::unavailable())?
                .attachments
                .insert(record.blob_id.clone(), record);
            drop(slot);
            return Ok(Json(json!({"data":attachment})).into_response());
        }
        let body = read_json(body).await?;
        if parts[3] == "download" {
            fields(&body, &["emailId", "blobId"])?;
            let attachment = client
                .download_attachment(text(&body["emailId"], 255)?, text(&body["blobId"], 255)?)
                .await?;
            self.inspect(&session).await?;
            let filename = attachment
                .name
                .chars()
                .take(200)
                .map(|c| {
                    if ['\r', '\n', '\0', '/', '\\'].contains(&c) {
                        '_'
                    } else {
                        c
                    }
                })
                .collect::<String>();
            let filename = if filename.is_empty() {
                "attachment"
            } else {
                &filename
            };
            let encoded =
                percent_encoding::utf8_percent_encode(filename, percent_encoding::NON_ALPHANUMERIC)
                    .to_string();
            let mut response = attachment.bytes.into_response();
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            );
            response.headers_mut().insert(
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&format!(
                    "attachment; filename=\"attachment\"; filename*=UTF-8''{encoded}"
                ))
                .map_err(|_| Failure::bad())?,
            );
            response.headers_mut().insert(
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static("default-src 'none'; sandbox"),
            );
            return Ok(response);
        }
        let data = match body["action"].as_str().ok_or_else(Failure::bad)? {
            "folders" => {
                fields(&body, &["action"])?;
                client.mailboxes().await?
            }
            "identities" => {
                fields(&body, &["action"])?;
                json!(
                    client
                        .identities()
                        .await?
                        .iter()
                        .filter(|r| r["email"] == access["senderAddress"])
                        .map(
                            |r| json!({"id":r["id"],"email":r["email"],"name":access["senderName"]})
                        )
                        .collect::<Vec<_>>()
                )
            }
            "list" => {
                fields(&body, &["action", "mailboxId", "text", "position", "limit"])?;
                let mut input = body.clone();
                input.as_object_mut().unwrap().shift_remove("action");
                client
                    .emails(
                        serde_json::from_value::<EmailQuery>(input).map_err(|_| Failure::bad())?,
                    )
                    .await?
            }
            "read" => {
                fields(&body, &["action", "emailId"])?;
                client.email(text(&body["emailId"], 255)?).await?
            }
            "update" => {
                fields(&body, &["action", "requestID", "emailId", "change"])?;
                let email = text(&body["emailId"], 255)?;
                let change: EmailChange =
                    serde_json::from_value(body["change"].clone()).map_err(|_| Failure::bad())?;
                self.write(
                    &session,
                    &body,
                    "update",
                    &json!({"emailId":email,"change":body["change"]}),
                    Some(email),
                    || async { Ok(client.update_email(email, change, None).await?) },
                )
                .await?
            }
            "draft" => {
                fields(&body, &["action", "requestID", "draft", "previousId"])?;
                let draft: MailDraft =
                    serde_json::from_value(body["draft"].clone()).map_err(|_| Failure::bad())?;
                let previous = body.get("previousId").map(|v| text(v, 255)).transpose()?;
                self.write(
                    &session,
                    &body,
                    "draft",
                    &body["draft"],
                    previous,
                    || async {
                        let previous = if let Some(previous) = previous {
                            Some(client.email(previous).await?)
                        } else {
                            None
                        };
                        for attachment in &draft.attachments {
                            let owned = session
                                .uploads
                                .lock()
                                .map_err(|_| Failure::unavailable())?
                                .attachments
                                .get(&attachment.blob_id)
                                .map(serde_json::to_value)
                                .transpose()
                                .map_err(|_| Failure::bad())?;
                            let previous_attachments =
                                previous.as_ref().and_then(|p| p["attachments"].as_array());
                            let owned = owned.as_ref().or_else(|| {
                                previous_attachments?
                                    .iter()
                                    .find(|v| v["blobId"] == attachment.blob_id)
                            });
                            if !owned.is_some_and(|o| {
                                o["name"] == attachment.name
                                    && o["type"] == attachment.media_type
                                    && o["size"].as_u64() == Some(attachment.size)
                            }) {
                                return Err(Failure {
                                    status: StatusCode::FORBIDDEN,
                                    message: "Mail attachment is not authorized for this session.",
                                    uncertain: false,
                                });
                            }
                        }
                        Ok(client
                            .create_draft(
                                draft,
                                body.get("previousId").map(|v| text(v, 255)).transpose()?,
                            )
                            .await?)
                    },
                )
                .await?
            }
            "submit" => {
                fields(&body, &["action", "requestID", "emailId", "identityId"])?;
                let response=self.options.control.send(json!({"action":"submit","token":session.token,"requestID":request_id(&body["requestID"])?,"emailID":text(&body["emailId"],255)?,"identityID":text(&body["identityId"],255)?})).await.map_err(|_|Failure::unknown())?;
                native_receipt(response)?
            }
            "submission-lookup" => {
                fields(&body, &["action", "emailId"])?;
                let value=self.options.control.send(json!({"action":"lookup","token":session.token,"emailID":text(&body["emailId"],255)?})).await.map_err(|_|Failure::unavailable())?;
                if value.is_null() {
                    Value::Null
                } else {
                    native_receipt(value)?
                }
            }
            "submission" => {
                fields(&body, &["action", "id", "operation"])?;
                let operation = text(&body["operation"], 16)?;
                if !["status", "cancel", "file"].contains(&operation) {
                    return Err(Failure::bad());
                }
                native_receipt(self.options.control.send(json!({"action":operation,"token":session.token,"id":record_id(&body["id"])?})).await.map_err(|_|Failure::unknown())?)?
            }
            _ => return Err(Failure::bad()),
        };
        // Do not return content obtained while the upstream request outlived its authority.
        self.inspect(&session).await?;
        Ok(Json(json!({"data":data})).into_response())
    }
}
async fn handle(State(state): State<Arc<Gateway>>, request: Request) -> Response {
    let origin = request.headers().get_all(header::ORIGIN).iter().count();
    let allowed = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .filter(|s| state.options.origins.contains(*s))
        .map(str::to_owned);
    let health = request.method() == Method::GET && request.uri().path() == "/healthz";
    let mut response = if health {
        Json(json!({"ok":true})).into_response()
    } else if origin != 1 || allowed.is_none() {
        (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"Invalid origin."})),
        )
            .into_response()
    } else if request.method() == Method::OPTIONS {
        let mut response = StatusCode::NO_CONTENT.into_response();
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("POST"),
        );
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("Content-Type, X-Webdock-Filename"),
        );
        response
    } else if request.method() != Method::POST {
        (
            StatusCode::METHOD_NOT_ALLOWED,
            Json(json!({"error":"Use POST."})),
        )
            .into_response()
    } else {
        match tokio::time::timeout(Duration::from_secs(60), state.route(request)).await {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => error.into_response(),
            Err(_) => Failure::unknown().into_response(),
        }
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if let Some(origin) = allowed
        && let Ok(origin) = HeaderValue::from_str(&origin)
    {
        response
            .headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            HeaderValue::from_static("true"),
        );
        response
            .headers_mut()
            .insert(header::VARY, HeaderValue::from_static("Origin"));
    }
    response
}
