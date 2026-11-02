use axum::{
    Json, Router,
    extract::{
        State,
        ws::{Message, WebSocketUpgrade},
    },
    http::{HeaderMap, header},
    response::IntoResponse,
    routing::{get, post},
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::Notify;
use webdock_database_gateway::{
    Error, Result,
    control::{Access, Control, Launch, RuntimeAccess},
    server::{Gateway, Options},
};
struct Authority {
    active: AtomicBool,
    used: AtomicBool,
    access: RuntimeAccess,
}
#[async_trait::async_trait]
impl Control for Authority {
    async fn exchange(&self, _: &str) -> Result<Launch> {
        if self.used.swap(true, Ordering::SeqCst) {
            return Err(Error::new(401, "Used code."));
        }
        Ok(Launch {
            token: "fixture-token".into(),
            scope_id: self.access.scope_id.clone(),
            expires_at: self.access.expires_at,
        })
    }
    async fn inspect(&self, token: &str) -> Result<Access> {
        if !self.active.load(Ordering::SeqCst) || token != "fixture-token" {
            return Err(Error::new(401, "Revoked."));
        }
        Ok(Access::Registered(self.access.clone()))
    }
    async fn logout(&self, _: &str) -> Result<()> {
        self.active.store(false, Ordering::SeqCst);
        Ok(())
    }
}
#[derive(Default)]
struct Upstream {
    started: Notify,
    cancelled: Notify,
    cancellations: AtomicUsize,
}
async fn query(
    State(state): State<Arc<Upstream>>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Json<Value> {
    assert_eq!(
        headers["x-tabularis-proxy-secret"],
        "synthetic-private-proxy-credential"
    );
    assert_eq!(input["connectionId"], "internal");
    if input["query"] == "select slow" {
        state.started.notify_one();
        state.cancelled.notified().await;
    }
    Json(json!({"ok":true,"data":{"rows":[[1]]}}))
}
async fn cancel(State(state): State<Arc<Upstream>>, Json(input): Json<Value>) -> Json<Value> {
    assert_eq!(input["connectionId"], "internal");
    assert!(input["queryRequestId"].is_string());
    state.cancellations.fetch_add(1, Ordering::SeqCst);
    state.cancelled.notify_waiters();
    Json(json!({"ok":true,"data":true}))
}
async fn session() -> axum::response::Response {
    let mut response=Json(json!({"apiVersion":"v1","authenticated":true,"csrfToken":"fixture-csrf","serverBuild":{"commit":"fixture"},"capabilities":{"uploads":true,"serverFileBrowser":true}})).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        "tabularis_session=fixture; HttpOnly; Path=/"
            .parse()
            .unwrap(),
    );
    response
}
async fn events(upgrade: WebSocketUpgrade) -> axum::response::Response {
    upgrade.on_upgrade(|mut socket|async move {while let Some(Ok(Message::Text(message)))=socket.recv().await{let value:Value=serde_json::from_str(&message).unwrap();socket.send(Message::Text(json!({"type":"fixture","payload":{"connectionId":"internal","order":value["order"]}}).to_string().into())).await.unwrap();}}).into_response()
}
#[tokio::test]
async fn actual_http_and_websocket_proxy_bind_connection_hide_secrets_and_cancel_revoked_queries() {
    proxy_flow(false).await;
}
#[tokio::test]
async fn private_ca_is_shared_by_https_and_websocket_transports_without_disabling_hostname_checks()
{
    proxy_flow(true).await;
}
struct TlsListener {
    listener: tokio::net::TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
}
impl axum::serve::Listener for TlsListener {
    type Io = tokio_rustls::server::TlsStream<tokio::net::TcpStream>;
    type Addr = std::net::SocketAddr;
    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let (socket, address) = self.listener.accept().await.unwrap();
            if let Ok(stream) = self.acceptor.accept(socket).await {
                return (stream, address);
            }
        }
    }
    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}
async fn proxy_flow(private_tls: bool) {
    let state = Arc::new(Upstream::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut trust = webdock_database_gateway::trust::Trust::from_pem(None).unwrap();
    let (upstream, tls) = if private_tls {
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let key = rcgen::KeyPair::generate().unwrap();
        let ca = params.self_signed(&key).unwrap();
        let issuer = rcgen::Issuer::new(params, key);
        let params = rcgen::CertificateParams::new(vec!["localhost".into()]).unwrap();
        let key = rcgen::KeyPair::generate().unwrap();
        let leaf = params.signed_by(&key, &issuer).unwrap();
        trust =
            webdock_database_gateway::trust::Trust::from_pem(Some(ca.pem().as_bytes())).unwrap();
        let key = rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der());
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![leaf.der().clone()], key.into())
        .unwrap();
        (
            format!("https://localhost:{port}"),
            Some(tokio_rustls::TlsAcceptor::from(Arc::new(config))),
        )
    } else {
        (format!("http://127.0.0.1:{port}"), None)
    };
    let router=Router::new().route("/api/v1/session",get(session)).route("/api/v1/rpc/execute_query",post(query)).route("/api/v1/rpc/cancel_query",post(cancel))
  .route("/api/v1/rpc/get_connections",post(||async{Json(json!({"ok":true,"data":[{"id":"internal","name":"Orders","params":{"driver":"postgres","database":"orders","password":"never-public","host":"private","username":"admin"}},{"id":"unrelated","name":"Other","params":{}}]}))}))
  .route("/api/v1/events",get(events)).with_state(state.clone());
    let (stop_backend, backend_shutdown) = tokio::sync::oneshot::channel();
    let backend = if let Some(acceptor) = tls {
        tokio::spawn(async move {
            axum::serve(TlsListener { listener, acceptor }, router)
                .with_graceful_shutdown(async {
                    let _ = backend_shutdown.await;
                })
                .await
                .unwrap();
        })
    } else {
        tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = backend_shutdown.await;
                })
                .await
                .unwrap();
        })
    };
    if private_tls {
        assert!(
            reqwest::get(format!("{upstream}/api/v1/session"))
                .await
                .is_err(),
            "untrusted runtime CA must fail"
        );
        let client = trust.http(reqwest::Client::builder()).build().unwrap();
        assert!(
            client
                .get(format!("https://127.0.0.1:{port}/api/v1/session"))
                .send()
                .await
                .is_err(),
            "a trusted CA must not disable hostname verification"
        );
    }
    let authority = Arc::new(Authority {
        active: AtomicBool::new(true),
        used: AtomicBool::new(false),
        access: RuntimeAccess {
            scope_id: webdock_core::id::RecordId::parse("123").unwrap(),
            subject: "fixture".into(),
            connection_id: "internal".into(),
            profile: webdock_database_gateway::policy::Profile::Read,
            runtime_origin: upstream.clone(),
            proxy_secret: "synthetic-private-proxy-credential".into(),
            expires_at: chrono::Utc::now() + chrono::Duration::minutes(1),
        },
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let gateway = Gateway::new_with_trust(
        Options {
            public_origin: origin.clone(),
            allowed_origins: vec!["http://localhost:3121".into()],
            runtime_origins: vec![upstream],
            expected_commit: Some("fixture".into()),
            control: authority.clone(),
            managed: None,
        },
        trust,
    )
    .unwrap();
    let router = gateway.router();
    let (stop_gateway, gateway_shutdown) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = gateway_shutdown.await;
            })
            .await
            .unwrap();
    });
    let client = reqwest::Client::new();
    let opened = client
        .post(format!("{origin}/session"))
        .header("origin", "http://localhost:3121")
        .json(&json!({"code":"a".repeat(43)}))
        .send()
        .await
        .unwrap();
    assert_eq!(opened.status(), 200);
    let cookie = opened.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    assert_eq!(
        client
            .post(format!("{origin}/session"))
            .header("origin", "http://localhost:3121")
            .json(&json!({"code":"a".repeat(43)}))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let negotiated: Value = client
        .get(format!("{origin}/s/123/api/v1/session"))
        .header("origin", "http://localhost:3121")
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(negotiated["capabilities"]["uploads"], false);
    assert_eq!(negotiated["capabilities"]["serverFileBrowser"], false);
    let value: Value = client
        .post(format!("{origin}/s/123/api/v1/rpc/get_connections"))
        .header("origin", "http://localhost:3121")
        .header("cookie", &cookie)
        .header("x-tabularis-csrf", "fixture-csrf")
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(value["data"].as_array().unwrap().len(), 1);
    assert_eq!(value["data"][0]["id"], "123");
    assert!(!value.to_string().contains("never-public"));
    assert!(
        !value["data"][0]["params"]
            .as_object()
            .unwrap()
            .contains_key("host")
    );
    assert_eq!(
        client
            .post(format!("{origin}/s/123/api/v1/rpc/execute_query"))
            .header("origin", "http://localhost:3121")
            .header("cookie", &cookie)
            .header("x-tabularis-csrf", "fixture-csrf")
            .json(&json!({"connectionId":"another","query":"select 1"}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{Message as WsMessage, client::IntoClientRequest};
    let mut request = origin.replacen("http://", "ws://", 1);
    request.push_str("/s/123/api/v1/events");
    let mut request = request.into_client_request().unwrap();
    request
        .headers_mut()
        .insert("origin", "http://localhost:3121".parse().unwrap());
    request
        .headers_mut()
        .insert("cookie", cookie.parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    for order in [1, 2] {
        socket
            .send(WsMessage::Text(
                json!({"type":"subscribe","order":order}).to_string().into(),
            ))
            .await
            .unwrap();
    }
    for order in [1, 2] {
        let message = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let event: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
        assert_eq!(event["payload"]["connectionId"], "123");
        assert_eq!(event["payload"]["order"], order);
    }
    let request = client
        .post(format!("{origin}/s/123/api/v1/rpc/execute_query"))
        .header("origin", "http://localhost:3121")
        .header("cookie", &cookie)
        .header("x-tabularis-csrf", "fixture-csrf")
        .json(&json!({"connectionId":"123","query":"select slow"}));
    let running = tokio::spawn(async move { request.send().await.unwrap() });
    state.started.notified().await;
    authority.active.store(false, Ordering::SeqCst);
    let response = tokio::time::timeout(std::time::Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), 401);
    for _ in 0..40 {
        if state.cancellations.load(Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(state.cancellations.load(Ordering::SeqCst) > 0);
    let closed = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
        .await
        .unwrap();
    assert!(closed.is_none_or(|value| value.is_err() || matches!(value, Ok(WsMessage::Close(_)))));
    gateway.shutdown().await;
    stop_gateway.send(()).unwrap();
    stop_backend.send(()).unwrap();
    server.await.unwrap();
    backend.await.unwrap();
}
