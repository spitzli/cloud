//! Browser gateway with live authorization, saved-connection scoping and bounded I/O.
use crate::{
    Error, Result,
    control::{Access, Control, RuntimeAccess, bounded_json, normalize_origin},
    managed::ManagedRuntimes,
    policy::{authorize_rpc, public_result},
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{
        Path, Request, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{any, get, post},
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;
const MAX_BODY: usize = 1_048_576;
const MAX_RESPONSE: usize = 8_388_608;
const COOKIE: &str = "webdock_database";
pub struct Options {
    pub public_origin: String,
    pub allowed_origins: Vec<String>,
    pub runtime_origins: Vec<String>,
    pub expected_commit: Option<String>,
    pub control: Arc<dyn Control>,
    pub managed: Option<Arc<dyn ManagedRuntimes>>,
}
pub struct Gateway {
    trust: crate::trust::Trust,
    stopped: AtomicBool,
    origin: String,
    authority: String,
    allowed: HashSet<String>,
    runtimes: HashSet<String>,
    expected_commit: Option<String>,
    control: Arc<dyn Control>,
    managed: Option<Arc<dyn ManagedRuntimes>>,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    capacity: Arc<Semaphore>,
    requests: Arc<Semaphore>,
    incoming: Arc<Semaphore>,
    websockets: Arc<Semaphore>,
    upgrades: Arc<Semaphore>,
    client: reqwest::Client,
}
struct Session {
    id: String,
    cookie: String,
    token: String,
    scope: String,
    expires: DateTime<Utc>,
    cancel: CancellationToken,
    target: Mutex<Option<String>>,
    sensitive: Mutex<Vec<String>>,
    negotiation: Mutex<Option<Negotiation>>,
    cancellations: Mutex<HashMap<String, CancelRequest>>,
    active: Arc<Semaphore>,
    queries: Arc<Semaphore>,
    _permit: OwnedSemaphorePermit,
    managed: Option<Arc<dyn ManagedRuntimes>>,
}
impl Drop for Session {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(manager) = self.managed.clone()
            && let Ok(handle) = tokio::runtime::Handle::try_current()
        {
            let id = self.id.clone();
            handle.spawn(async move {
                manager.release(&id).await;
            });
        }
    }
}
#[derive(Clone)]
struct Resolved {
    access: RuntimeAccess,
    authentication_origin: String,
}
#[derive(Clone)]
struct Negotiation {
    cookie: String,
    csrf: String,
    public: Value,
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let mut response=(StatusCode::from_u16(self.status).unwrap_or(StatusCode::BAD_GATEWAY),Json(json!({"ok":false,"error":{"code":"DATABASE_GATEWAY_ERROR","message":self.message,"details":null,"requestId":uuid::Uuid::new_v4().to_string()}}))).into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response
    }
}
fn expired() -> Error {
    Error::new(401, "Database access expired or was revoked.")
}
fn transport_error() -> Error {
    Error::new(
        502,
        "Database request failed. Check its outcome before retrying.",
    )
}
fn permit(semaphore: &Arc<Semaphore>) -> Result<OwnedSemaphorePermit> {
    semaphore
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::new(429, "Too many concurrent database operations."))
}
fn body_json(headers: &HeaderMap, body: &[u8]) -> Result<Value> {
    if headers.get_all(header::CONTENT_TYPE).iter().count() != 1
        || headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(str::trim)
            != Some("application/json")
    {
        return Err(Error::new(415, "Use application/json."));
    }
    if body.len() > MAX_BODY {
        return Err(Error::new(413, "Database request is too large."));
    }
    serde_json::from_slice(body).map_err(|_| Error::new(400, "Invalid JSON."))
}
fn object(value: &Value) -> Result<&serde_json::Map<String, Value>> {
    value
        .as_object()
        .ok_or(Error::new(400, "Invalid database request."))
}
fn safe_header(value: &str) -> Result<HeaderValue> {
    HeaderValue::from_str(value).map_err(|_| transport_error())
}
fn private_header(value: &str) -> Result<HeaderValue> {
    let mut value = safe_header(value)?;
    value.set_sensitive(true);
    Ok(value)
}
impl Gateway {
    pub fn new(options: Options) -> Result<Arc<Self>> {
        Self::new_with_trust(options, crate::trust::Trust::from_pem(None)?)
    }
    pub fn new_with_trust(options: Options, trust: crate::trust::Trust) -> Result<Arc<Self>> {
        let origin = normalize_origin(&options.public_origin)?;
        let url = url::Url::parse(&origin).map_err(|_| transport_error())?;
        let authority = url[url::Position::BeforeHost..url::Position::AfterPort].to_owned();
        let allowed = options
            .allowed_origins
            .iter()
            .map(|s| normalize_origin(s))
            .collect::<Result<HashSet<_>>>()?;
        if allowed.is_empty() {
            return Err(Error::new(503, "Browser origins are required."));
        }
        let runtimes = options
            .runtime_origins
            .iter()
            .map(|s| normalize_origin(s))
            .collect::<Result<HashSet<_>>>()?;
        let client = trust
            .http(reqwest::Client::builder())
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(35))
            .build()
            .map_err(|_| transport_error())?;
        Ok(Arc::new(Self {
            trust,
            stopped: AtomicBool::new(false),
            origin,
            authority,
            allowed,
            runtimes,
            expected_commit: options.expected_commit,
            control: options.control,
            managed: options.managed,
            sessions: Mutex::new(HashMap::new()),
            capacity: Arc::new(Semaphore::new(256)),
            requests: Arc::new(Semaphore::new(64)),
            incoming: Arc::new(Semaphore::new(64)),
            websockets: Arc::new(Semaphore::new(64)),
            upgrades: Arc::new(Semaphore::new(16)),
            client,
        }))
    }
    pub fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .route("/healthz", get(|| async { Json(json!({"ok":true})) }))
            .route("/session", post(launch))
            .route("/s/{scope}/api/v1/events", get(events))
            .route("/s/{scope}/api/v1/{*operation}", any(dispatch))
            .fallback(|| async { Error::new(404, "Database endpoint unavailable.") })
            .method_not_allowed_fallback(|| async { Error::new(405, "Method not allowed.") })
            .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY))
            .layer(middleware::from_fn_with_state(self.clone(), boundary))
            .with_state(self.clone())
    }
    pub async fn shutdown(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.capacity.close();
        self.requests.close();
        self.incoming.close();
        self.websockets.close();
        self.upgrades.close();
        let sessions = std::mem::take(&mut *self.sessions.lock().await);
        futures_util::future::join_all(sessions.values().map(|session| self.retire(session))).await;
        if let Some(managed) = &self.managed {
            managed.shutdown().await;
        }
    }
    async fn retire(&self, session: &Session) {
        session.cancel.cancel();
        self.sessions.lock().await.remove(&session.cookie);
        let cancellations = session
            .cancellations
            .lock()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        futures_util::future::join_all(cancellations.iter().map(CancelRequest::send)).await;
        if let Some(managed) = &self.managed {
            managed.release(&session.id).await;
        }
    }
    async fn inspect(&self, session: &Session) -> Result<Resolved> {
        let result = self.inspect_inner(session).await;
        if result.is_err() {
            self.retire(session).await;
        }
        result
    }
    async fn inspect_inner(&self, session: &Session) -> Result<Resolved> {
        if session.cancel.is_cancelled() || session.expires <= Utc::now() {
            return Err(expired());
        }
        let access = self
            .control
            .inspect(&session.token)
            .await
            .map_err(|_| expired())?;
        access.validate()?;
        if session.cancel.is_cancelled()
            || access.scope_id() != session.scope
            || access.expires_at() <= Utc::now()
        {
            return Err(expired());
        }
        let (identity, resolved) = match access {
            Access::Registered(access) => {
                if !self.runtimes.contains(&access.runtime_origin) {
                    return Err(Error::new(403, "Database runtime is not authorized."));
                }
                let identity = json!([
                    "registered",
                    access.scope_id,
                    access.subject,
                    access.connection_id,
                    access.profile,
                    access.runtime_origin,
                    access.proxy_secret
                ])
                .to_string();
                let authentication_origin = access.runtime_origin.clone();
                (
                    identity,
                    Resolved {
                        access,
                        authentication_origin,
                    },
                )
            }
            Access::Managed(access) => {
                let manager = self
                    .managed
                    .as_ref()
                    .ok_or(Error::new(503, "Managed database runtime unavailable."))?;
                let identity = format!("managed:{}", crate::managed::managed_identity(&access));
                *session.sensitive.lock().await = vec![
                    access.database.password.clone(),
                    access.database.host.clone(),
                    access.database.username.clone(),
                    access.database.ca_certificate.clone(),
                ];
                let resolved = manager.acquire(&session.id, &access).await?;
                (
                    identity,
                    Resolved {
                        access: resolved.access,
                        authentication_origin: resolved.authentication_origin,
                    },
                )
            }
        };
        let mut target = session.target.lock().await;
        if target.as_ref().is_some_and(|old| old != &identity) {
            return Err(expired());
        }
        *target = Some(identity);
        if session.cancel.is_cancelled() || session.expires <= Utc::now() {
            return Err(expired());
        }
        Ok(resolved)
    }
    async fn locate(&self, scope: &str, headers: &HeaderMap) -> Result<Arc<Session>> {
        webdock_core::id::RecordId::parse(scope).map_err(|_| expired())?;
        let mut cookies = Vec::new();
        for raw in headers.get_all(header::COOKIE).iter() {
            let raw = raw.to_str().map_err(|_| expired())?;
            cookies.extend(
                raw.split(';')
                    .map(str::trim)
                    .filter_map(|v| v.strip_prefix("webdock_database=")),
            );
        }
        if cookies.len() != 1 {
            return Err(Error::new(401, "Database session required."));
        }
        let session = self
            .sessions
            .lock()
            .await
            .get(cookies[0])
            .cloned()
            .ok_or_else(expired)?;
        if session.scope != scope || session.cancel.is_cancelled() {
            return Err(expired());
        }
        Ok(session)
    }
    fn upstream_headers(
        &self,
        resolved: &Resolved,
        negotiation: Option<&Negotiation>,
    ) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        headers.insert(
            header::ORIGIN,
            safe_header(&resolved.authentication_origin)?,
        );
        headers.insert("x-tabularis-user", safe_header(&resolved.access.subject)?);
        headers.insert(
            "x-tabularis-proxy-secret",
            private_header(&resolved.access.proxy_secret)?,
        );
        headers.insert("x-tabularis-deadline-ms", HeaderValue::from_static("30000"));
        if let Some(negotiation) = negotiation {
            headers.insert(header::COOKIE, private_header(&negotiation.cookie)?);
            headers.insert("x-tabularis-csrf", private_header(&negotiation.csrf)?);
        }
        Ok(headers)
    }
    async fn negotiate(&self, session: &Session, resolved: &Resolved) -> Result<Negotiation> {
        let mut slot = session.negotiation.lock().await;
        if let Some(negotiation) = slot.as_ref() {
            return Ok(negotiation.clone());
        }
        let response = self
            .client
            .get(format!("{}/api/v1/session", resolved.access.runtime_origin))
            .headers(self.upstream_headers(resolved, None)?)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|_| transport_error())?;
        if !response.status().is_success() {
            return Err(Error::new(502, "Database runtime authentication failed."));
        }
        let cookies = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .filter(|v| v.starts_with("tabularis_session="))
            .map(|v| v.split(';').next().unwrap_or("").to_owned())
            .collect::<Vec<_>>();
        if cookies.len() != 1 {
            return Err(Error::new(502, "Database runtime did not issue a session."));
        }
        let mut data = bounded_json(response, MAX_RESPONSE).await?;
        let csrf = data
            .get("csrfToken")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty() && v.len() <= 4096)
            .ok_or(Error::new(502, "Incompatible database runtime."))?
            .to_owned();
        if data["apiVersion"] != "v1"
            || data["authenticated"] != true
            || self.expected_commit.as_ref().is_some_and(|commit| {
                data["serverBuild"]["commit"].as_str() != Some(commit.as_str())
            })
        {
            return Err(Error::new(502, "Incompatible database runtime."));
        }
        let mut capabilities = data
            .get("capabilities")
            .map(object)
            .transpose()?
            .cloned()
            .unwrap_or_default();
        for (name, enabled) in [
            ("rpc", true),
            ("events", true),
            ("uploads", false),
            ("downloads", false),
            ("pluginAssets", false),
            ("mcpHostConfiguration", false),
            ("serverFileBrowser", false),
            ("nativeUpdater", false),
        ] {
            capabilities.insert(name.into(), json!(enabled));
        }
        data["capabilities"] = Value::Object(capabilities);
        data["access"] =
            json!({"remote":true,"authorizationLevel":"database","highRiskCapabilities":false});
        let mut policy = data
            .get("queryResponsePolicy")
            .map(object)
            .transpose()?
            .cloned()
            .unwrap_or_default();
        policy.insert("maxResponseBytes".into(), json!(MAX_RESPONSE));
        data["queryResponsePolicy"] = Value::Object(policy);
        let negotiation = Negotiation {
            cookie: cookies[0].clone(),
            csrf,
            public: data,
        };
        *slot = Some(negotiation.clone());
        Ok(negotiation)
    }
}
async fn boundary(State(gateway): State<Arc<Gateway>>, request: Request, next: Next) -> Response {
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let health = request.uri().path() == "/healthz" && request.method() == Method::GET;
    let valid = health
        || (request.headers().get_all(header::HOST).iter().count() == 1
            && request
                .headers()
                .get(header::HOST)
                .and_then(|v| v.to_str().ok())
                == Some(gateway.authority.as_str())
            && request.headers().get_all(header::ORIGIN).iter().count() == 1
            && origin.as_ref().is_some_and(|v| gateway.allowed.contains(v)));
    let mut response = if !valid {
        Error::new(403, "Invalid origin.").into_response()
    } else if request.method() == Method::OPTIONS {
        StatusCode::NO_CONTENT.into_response()
    } else if health {
        next.run(request).await
    } else {
        bounded_request(&gateway, request, next)
            .await
            .unwrap_or_else(IntoResponse::into_response)
    };
    if let Some(origin) = origin.filter(|v| gateway.allowed.contains(v)) {
        response
            .headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin.parse().unwrap());
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            HeaderValue::from_static("true"),
        );
        response
            .headers_mut()
            .insert(header::VARY, HeaderValue::from_static("Origin"));
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST, OPTIONS"),
        );
        response.headers_mut().insert(header::ACCESS_CONTROL_ALLOW_HEADERS,HeaderValue::from_static("content-type,x-request-id,x-tabularis-csrf,x-tabularis-deadline-ms,x-tabularis-cancellation-id"));
    }
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
}
async fn bounded_request(gateway: &Arc<Gateway>, request: Request, next: Next) -> Result<Response> {
    let _incoming = permit(&gateway.incoming)?;
    if let Some(scope) = request
        .uri()
        .path()
        .strip_prefix("/s/")
        .and_then(|path| path.split('/').next())
    {
        gateway.locate(scope, request.headers()).await?;
    }
    let (parts, body) = request.into_parts();
    let body = tokio::time::timeout(
        Duration::from_secs(15),
        axum::body::to_bytes(body, MAX_BODY),
    )
    .await
    .map_err(|_| Error::new(408, "Database request timed out."))?
    .map_err(|_| Error::new(413, "Database request is too large."))?;
    let request = Request::from_parts(parts, axum::body::Body::from(body));
    tokio::time::timeout(Duration::from_secs(60), next.run(request))
        .await
        .map_err(|_| transport_error())
}
async fn launch(
    State(gateway): State<Arc<Gateway>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response> {
    let value = body_json(&headers, &body)?;
    let value = object(&value)?;
    let code = value
        .get("code")
        .and_then(Value::as_str)
        .filter(|v| {
            v.len() == 43
                && v.bytes()
                    .all(|v| v.is_ascii_alphanumeric() || b"_-".contains(&v))
        })
        .ok_or(Error::new(400, "Invalid launch code."))?;
    if value.len() != 1 {
        return Err(Error::new(400, "Invalid launch code."));
    }
    let slot = gateway
        .capacity
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::new(503, "Database gateway is at capacity."))?;
    let cookie = crate::random_token()?;
    let exchange = gateway
        .control
        .exchange(code)
        .await
        .map_err(|_| Error::new(401, "Launch code expired or was already used."))?;
    if exchange.token.is_empty() || exchange.token.len() > 4096 || exchange.expires_at <= Utc::now()
    {
        return Err(expired());
    }
    let session = Arc::new(Session {
        id: uuid::Uuid::new_v4().to_string(),
        cookie: cookie.clone(),
        token: exchange.token,
        scope: exchange.scope_id.to_string(),
        expires: exchange
            .expires_at
            .min(Utc::now() + chrono::Duration::minutes(15)),
        cancel: CancellationToken::new(),
        target: Mutex::new(None),
        sensitive: Mutex::new(vec![]),
        negotiation: Mutex::new(None),
        cancellations: Mutex::new(HashMap::new()),
        active: Arc::new(Semaphore::new(16)),
        queries: Arc::new(Semaphore::new(4)),
        _permit: slot,
        managed: gateway.managed.clone(),
    });
    gateway.inspect(&session).await?;
    {
        let mut sessions = gateway.sessions.lock().await;
        if gateway.stopped.load(Ordering::SeqCst) {
            return Err(Error::new(503, "Database gateway is shutting down."));
        }
        sessions.insert(cookie.clone(), session.clone());
    }
    let weak_gateway = Arc::downgrade(&gateway);
    let weak_session = Arc::downgrade(&session);
    let cancelled = session.cancel.clone();
    let expiry = (session.expires - Utc::now()).to_std().unwrap_or_default();
    tokio::spawn(async move {
        tokio::select! {_=cancelled.cancelled()=>{},_=tokio::time::sleep(expiry)=>{}}
        if let (Some(gateway), Some(session)) = (weak_gateway.upgrade(), weak_session.upgrade()) {
            gateway.retire(&session).await;
        }
    });
    if !session.sensitive.lock().await.is_empty() {
        let weak_gateway = Arc::downgrade(&gateway);
        let weak_session = Arc::downgrade(&session);
        let cancel = session.cancel.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(3));
            interval.tick().await;
            loop {
                tokio::select! {_=cancel.cancelled()=>break,_=interval.tick()=>{let (Some(gateway),Some(session))=(weak_gateway.upgrade(),weak_session.upgrade())else{break;};if gateway.inspect(&session).await.is_err(){break;}}}
            }
        });
    }
    let mut response=Json(json!({"scopeID":session.scope,"connectionID":session.scope,"baseUrl":format!("{}/s/{}",gateway.origin,session.scope),"expiresAt":session.expires.to_rfc3339_opts(chrono::SecondsFormat::Millis,true)})).into_response();
    response.headers_mut().append(
        header::SET_COOKIE,
        safe_header(&format!(
            "{COOKIE}={cookie}; Path=/s/{}/; HttpOnly; SameSite=Strict; Max-Age={}{}",
            session.scope,
            (session.expires - Utc::now()).num_seconds().clamp(0, 900),
            if gateway.origin.starts_with("https:") {
                "; Secure"
            } else {
                ""
            }
        ))?,
    );
    Ok(response)
}
#[derive(Clone)]
struct CancelRequest {
    client: reqwest::Client,
    origin: String,
    headers: HeaderMap,
    connection: String,
    request_id: String,
}
impl CancelRequest {
    async fn send(&self) {
        let _ = self
            .client
            .post(format!("{}/api/v1/rpc/cancel_query", self.origin))
            .headers(self.headers.clone())
            .json(&json!({"connectionId":self.connection,"queryRequestId":self.request_id}))
            .timeout(Duration::from_secs(2))
            .send()
            .await;
    }
}
struct QueryCancellation {
    armed: bool,
    request: CancelRequest,
    session: std::sync::Weak<Session>,
    key: String,
}
impl Drop for QueryCancellation {
    fn drop(&mut self) {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let request = self.request.clone();
            let session = self.session.clone();
            let key = self.key.clone();
            let armed = self.armed;
            handle.spawn(async move {
                if let Some(session) = session.upgrade() {
                    session.cancellations.lock().await.remove(&key);
                }
                if armed {
                    request.send().await;
                }
            });
        }
    }
}
struct RpcAudit {
    scope: String,
    subject: String,
    command: String,
    started: std::time::Instant,
    status: u16,
}
impl Drop for RpcAudit {
    fn drop(&mut self) {
        tracing::info!(event="database.rpc",scope_id=%self.scope,subject=%self.subject,command=%self.command,status=self.status,duration_ms=self.started.elapsed().as_millis() as u64);
    }
}
fn redact(value: &mut Value, secrets: &[String]) {
    match value {
        Value::String(text) => {
            for secret in secrets.iter().filter(|v| !v.is_empty()) {
                *text = text.replace(secret, "[redacted]");
            }
        }
        Value::Array(values) => {
            for value in values {
                redact(value, secrets)
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                redact(value, secrets)
            }
        }
        _ => {}
    }
}
async fn dispatch(
    State(gateway): State<Arc<Gateway>>,
    Path((scope, operation)): Path<(String, String)>,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response> {
    let session = gateway.locate(&scope, &headers).await?;
    let _session_permit = permit(&session.active)?;
    let _global_permit = permit(&gateway.requests)?;
    let access = gateway.inspect(&session).await?;
    let negotiation = gateway.negotiate(&session, &access).await?;
    if operation == "session" && method == Method::GET {
        return Ok(Json(negotiation.public).into_response());
    }
    if method != Method::POST {
        return Err(Error::new(405, "Method not allowed."));
    }
    if headers.get_all("x-tabularis-csrf").iter().count() != 1
        || headers
            .get("x-tabularis-csrf")
            .and_then(|v| v.to_str().ok())
            != Some(negotiation.csrf.as_str())
    {
        return Err(Error::new(403, "Invalid database CSRF token."));
    }
    let upstream_headers = gateway.upstream_headers(&access, Some(&negotiation))?;
    if operation == "logout" {
        gateway.retire(&session).await;
        let _ = gateway
            .client
            .post(format!("{}/api/v1/logout", access.access.runtime_origin))
            .headers(upstream_headers)
            .timeout(Duration::from_secs(2))
            .send()
            .await;
        gateway.control.logout(&session.token).await?;
        return Ok(Json(json!({"ok":true})).into_response());
    }
    let command = operation
        .strip_prefix("rpc/")
        .filter(|v| {
            !v.is_empty()
                && v.len() <= 120
                && v.bytes().all(|c| c.is_ascii_lowercase() || c == b'_')
        })
        .ok_or(Error::new(403, "This database endpoint is not permitted."))?;
    let mut audit = RpcAudit {
        scope: session.scope.clone(),
        subject: access.access.subject.clone(),
        command: command.into(),
        started: std::time::Instant::now(),
        status: 499,
    };
    let outcome=async {
    let input = body_json(&headers, &body)?;
    let mut input = authorize_rpc(
        command,
        input,
        &session.scope,
        &access.access.connection_id,
        access.access.profile,
    )?;
    let query = [
        "test_connection",
        "execute_query",
        "execute_query_batch",
        "count_query",
        "explain_query_plan",
    ]
    .contains(&command);
    let _query_permit = if query {
        Some(permit(&session.queries)?)
    } else {
        None
    };
    let upstream_command = if command == "test_connection" {
        input = json!({"connectionId":access.access.connection_id,"query":"SELECT 1","limit":1});
        "execute_query"
    } else {
        command
    };
    let mut upstream_headers = upstream_headers;
    let mut request_id = uuid::Uuid::new_v4().to_string();
    for name in ["x-request-id", "x-tabularis-cancellation-id"] {
        if let Some(value) = headers.get(name).and_then(|v| v.to_str().ok()).filter(|v| {
            !v.is_empty()
                && v.len() <= 128
                && v.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        }) {
            upstream_headers.insert(name, safe_header(value)?);
            if name == "x-request-id" {
                request_id = value.into();
            }
        }
    }
    upstream_headers.insert("x-request-id", safe_header(&request_id)?);
    let request=CancelRequest {client:gateway.client.clone(),origin:access.access.runtime_origin.clone(),headers:upstream_headers.clone(),connection:access.access.connection_id.clone(),request_id};
    let key=uuid::Uuid::new_v4().to_string();
    let mut cancellation=QueryCancellation{armed:query,request:request.clone(),session:Arc::downgrade(&session),key:key.clone()};
    if query{session.cancellations.lock().await.insert(key,request);}
    let operation = async {
        let response = gateway
            .client
            .post(format!(
                "{}/api/v1/rpc/{upstream_command}",
                access.access.runtime_origin
            ))
            .headers(upstream_headers)
            .json(&input)
            .send()
            .await
            .map_err(|_| transport_error())?;
        let status = response.status();
        let value = bounded_json(response, MAX_RESPONSE).await?;
        Ok::<_, Error>((status, value))
    };
    tokio::pin!(operation);
    let mut interval = tokio::time::interval(Duration::from_secs(3));
    interval.tick().await;
    let end=tokio::time::Instant::now()+Duration::from_secs(35);
    let deadline=tokio::time::sleep_until(end);
    tokio::pin!(deadline);
    let (status, mut result) = loop {
        tokio::select! {
            _=session.cancel.cancelled()=>return Err(expired()),_= &mut deadline=>return Err(transport_error()),
            value=&mut operation=>break value?,_=interval.tick()=>{tokio::time::timeout_at(end,gateway.inspect(&session)).await.map_err(|_|transport_error())??;}
        }
    };
    tokio::time::timeout_at(end,gateway.inspect(&session)).await.map_err(|_|transport_error())??;
    let row = result.as_object_mut().ok_or_else(transport_error)?;
    if row.get("ok") == Some(&Value::Bool(false))
        && let Some(error) = row.get_mut("error")
    {
        redact(error, &session.sensitive.lock().await);
    }
    if row.get("ok") == Some(&Value::Bool(true)) {
        let value = row.remove("data").unwrap_or(Value::Null);
        row.insert(
            "data".into(),
            if command == "test_connection" {
                json!("Connection successful!")
            } else {
                public_result(
                    command,
                    value,
                    &session.scope,
                    &access.access.connection_id,
                    access.access.profile,
                )?
            },
        );
    }
    cancellation.armed = false;
    Ok((
        StatusCode::from_u16(status.as_u16()).map_err(|_| transport_error())?,
        Json(result),
    )
        .into_response())
    }.await;
    audit.status = match &outcome {
        Ok(response) => response.status().as_u16(),
        Err(error) => error.status,
    };
    outcome
}
async fn events(
    State(gateway): State<Arc<Gateway>>,
    Path(scope): Path<String>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response> {
    let _upgrade = permit(&gateway.upgrades)?;
    let client_permit = permit(&gateway.websockets)?;
    let session = gateway.locate(&scope, &headers).await?;
    let access = gateway.inspect(&session).await?;
    let negotiation = gateway.negotiate(&session, &access).await?;
    Ok(upgrade
        .max_message_size(MAX_BODY)
        .max_frame_size(MAX_BODY)
        .max_write_buffer_size(MAX_RESPONSE)
        .on_upgrade(move |socket| async move {
            let _permit = client_permit;
            forward_events(gateway, session, access, negotiation, socket).await
        })
        .into_response())
}
async fn forward_events(
    gateway: Arc<Gateway>,
    session: Arc<Session>,
    access: Resolved,
    negotiation: Negotiation,
    mut client: WebSocket,
) {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{
        Message as UpstreamMessage, client::IntoClientRequest, protocol::WebSocketConfig,
    };
    let result=async {
        let mut target=url::Url::parse(&format!("{}/api/v1/events",access.access.runtime_origin)).map_err(|_|transport_error())?;
        let scheme=if target.scheme()=="https"{"wss"}else{"ws"};target.set_scheme(scheme).map_err(|_|transport_error())?;
        let mut request=target.as_str().into_client_request().map_err(|_|transport_error())?;
        for (name,value) in gateway.upstream_headers(&access,Some(&negotiation))?.iter(){request.headers_mut().insert(name.clone(),value.clone());}
        let config=WebSocketConfig::default().max_message_size(Some(MAX_RESPONSE)).max_frame_size(Some(MAX_RESPONSE)).max_write_buffer_size(MAX_BODY);
        let (mut upstream,_)=tokio::time::timeout(Duration::from_secs(10),tokio_tungstenite::connect_async_tls_with_config(request,Some(config),false,Some(gateway.trust.websocket()))).await.map_err(|_|transport_error())?.map_err(|_|transport_error())?;
        let mut interval=tokio::time::interval(Duration::from_secs(3));interval.tick().await;
        loop {tokio::select! {
            _=session.cancel.cancelled()=>break,
            _=interval.tick()=>{gateway.inspect(&session).await?;},
            message=client.recv()=>{
                let Some(Ok(message))=message else{break;};
                match message {
                    Message::Text(text)=>{
                        let value:Value=serde_json::from_str(&text).map_err(|_|transport_error())?;let value=object(&value)?;
                        if !matches!(value.get("type").and_then(Value::as_str),Some("subscribe"|"unsubscribe"|"ping")){return Err(transport_error());}
                        gateway.inspect(&session).await?;
                        tokio::time::timeout(Duration::from_secs(10),upstream.send(UpstreamMessage::Text(serde_json::to_string(value).map_err(|_|transport_error())?.into()))).await.map_err(|_|transport_error())?.map_err(|_|transport_error())?;
                    }
                    Message::Close(_)=>break,Message::Ping(value)=>{client.send(Message::Pong(value)).await.map_err(|_|transport_error())?;},Message::Pong(_)=>{},Message::Binary(_)=>return Err(transport_error()),
                }
            },
            message=upstream.next()=>{
                let Some(Ok(message))=message else{break;};
                match message {
                    UpstreamMessage::Text(text)=>{
                        let mut event:Value=serde_json::from_str(&text).map_err(|_|transport_error())?;object(&event)?;
                        if let Some(payload)=event.get_mut("payload").and_then(Value::as_object_mut){for field in ["connectionId","connection_id"]{if payload.get(field).and_then(Value::as_str)==Some(access.access.connection_id.as_str()){payload.insert(field.into(),json!(session.scope));}}}
                        gateway.inspect(&session).await?;
                        let text=serde_json::to_string(&event).map_err(|_|transport_error())?;if text.len()>MAX_RESPONSE{return Err(transport_error());}
                        tokio::time::timeout(Duration::from_secs(10),client.send(Message::Text(text.into()))).await.map_err(|_|transport_error())?.map_err(|_|transport_error())?;
                    }
                    UpstreamMessage::Close(_)=>break,UpstreamMessage::Ping(value)=>{upstream.send(UpstreamMessage::Pong(value)).await.map_err(|_|transport_error())?;},UpstreamMessage::Pong(_)=>{},_=>return Err(transport_error()),
                }
            }
        }}
        let _=upstream.close(None).await;Ok::<(),Error>(())
    }.await;
    let _ = result;
    let _ = tokio::time::timeout(
        Duration::from_secs(1),
        client.send(Message::Close(Some(axum::extract::ws::CloseFrame {
            code: 1008,
            reason: "Database session ended".into(),
        }))),
    )
    .await;
}
