#![cfg(feature = "test-support")]
use axum::{
    Json, Router,
    body::Body,
    http::{Request, StatusCode},
    routing::post,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tower::ServiceExt;
use webdock_mail::gateway::{Control, ControlFailure, Gateway, GatewayOptions};
struct MalformedReceipt(FixtureControl);
#[async_trait::async_trait]
impl Control for MalformedReceipt {
    async fn call(&self, input: Value) -> Result<Value, ControlFailure> {
        self.0.call(input).await
    }
    async fn send(&self, _input: Value) -> Result<Value, ControlFailure> {
        Ok(json!({"id":"invalid/id","state":"queued"}))
    }
}
#[tokio::test]
async fn malformed_queue_acknowledgement_remains_uncertain() {
    let control = Arc::new(MalformedReceipt(FixtureControl {
        revoked: AtomicBool::new(false),
        runtime: "http://app-12.wd-mail-12.svc.cluster.local:8080".into(),
        submissions: AtomicUsize::new(0),
    }));
    let router = Gateway::new(
        GatewayOptions::new(vec!["https://studio.webdock.test".into()], control).unwrap(),
    )
    .unwrap()
    .router();
    let launch = request(
        &router,
        "/session",
        json!({"code":"x".repeat(43)}),
        "",
        "https://studio.webdock.test",
    )
    .await;
    let cookie = launch.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let response=request(&router,"/s/123/mail",json!({"action":"submit","requestID":uuid::Uuid::new_v4(),"emailId":"draft","identityId":"sender"}),&cookie,"https://studio.webdock.test").await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        serde_json::from_slice::<Value>(&response.into_body().collect().await.unwrap().to_bytes())
            .unwrap()["uncertain"],
        true
    );
}
struct FixtureControl {
    revoked: AtomicBool,
    runtime: String,
    submissions: AtomicUsize,
}
#[async_trait::async_trait]
impl Control for FixtureControl {
    async fn call(&self, input: Value) -> Result<Value, ControlFailure> {
        if self.revoked.load(Ordering::SeqCst) {
            return Err(ControlFailure);
        }
        let expires = (chrono::Utc::now() + chrono::Duration::minutes(1)).to_rfc3339();
        Ok(match input["action"].as_str().unwrap() {
            "exchange" => {
                json!({"token":"t".repeat(43),"scopeID":"123","customerID":"12","mailboxID":"34","expiresAt":expires})
            }
            "inspect" => {
                json!({"scopeID":"123","customerID":"12","mailboxID":"34","subject":"56","accountID":"account-a","senderName":"Support","senderAddress":"support@example.test","runtimeOrigin":self.runtime,"authorization":"Basic private","expiresAt":expires,"grantRevision":"1"})
            }
            "logout" => json!({"ok":true}),
            _ => panic!("unexpected control action"),
        })
    }
    async fn send(&self, input: Value) -> Result<Value, ControlFailure> {
        assert_eq!(input["token"], "t".repeat(43));
        self.submissions.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"id":"789","state":"queued","filed":false,"authorization":"must-not-leak"}))
    }
}
async fn request(
    router: &Router,
    path: &str,
    value: Value,
    cookie: &str,
    origin: &str,
) -> axum::response::Response {
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("origin", origin)
                .header("content-type", "application/json")
                .header("cookie", cookie)
                .body(Body::from(value.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}
#[tokio::test]
async fn scope_cookie_origin_revocation_credentials_and_queue_only_submission() {
    let reads = Arc::new(AtomicUsize::new(0));
    let counter = reads.clone();
    let upstream=Router::new().route("/jmap",post(move |headers:axum::http::HeaderMap,Json(body):Json<Value>|{let counter=counter.clone();async move{assert_eq!(headers["authorization"],"Basic private");let call=&body["methodCalls"][0];assert_eq!(call[0],"Mailbox/get");assert_eq!(call[1]["accountId"],"account-a");counter.fetch_add(1,Ordering::SeqCst);Json(json!({"methodResponses":[[call[0],{"accountId":"account-a","list":[{"id":"inbox","name":"Inbox"}]},call[2]]]}))}}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let runtime = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let control = Arc::new(FixtureControl {
        revoked: AtomicBool::new(false),
        runtime: "http://app-12.wd-mail-12.svc.cluster.local:8080".into(),
        submissions: AtomicUsize::new(0),
    });
    let mut options =
        GatewayOptions::new(vec!["https://studio.webdock.test".into()], control.clone()).unwrap();
    options.test_runtime_origin = Some(runtime);
    let router = Gateway::new(options).unwrap().router();
    assert_eq!(
        request(
            &router,
            "/session",
            json!({"code":"x".repeat(43)}),
            "",
            "https://evil.test"
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let launch = request(
        &router,
        "/session",
        json!({"code":"x".repeat(43)}),
        "",
        "https://studio.webdock.test",
    )
    .await;
    assert_eq!(launch.status(), StatusCode::OK);
    let cookie = launch.headers()["set-cookie"].to_str().unwrap().to_owned();
    assert!(
        cookie.contains("HttpOnly") && cookie.contains("Secure") && cookie.contains("Path=/s/123/")
    );
    let cookie = cookie.split(';').next().unwrap();
    let body = launch.into_body().collect().await.unwrap().to_bytes();
    assert!(!String::from_utf8_lossy(&body).contains("token"));
    let response = request(
        &router,
        "/s/123/mail",
        json!({"action":"folders"}),
        cookie,
        "https://studio.webdock.test",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        !String::from_utf8_lossy(&response.into_body().collect().await.unwrap().to_bytes())
            .contains("private")
    );
    assert_eq!(
        request(
            &router,
            "/s/124/mail",
            json!({"action":"folders"}),
            cookie,
            "https://studio.webdock.test"
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &router,
            "/s/123/mail",
            json!({"action":"folders","accountId":"foreign"}),
            cookie,
            "https://studio.webdock.test"
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let duplicate = format!("{cookie}; {cookie}");
    assert_eq!(
        request(
            &router,
            "/s/123/mail",
            json!({"action":"folders"}),
            &duplicate,
            "https://studio.webdock.test"
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let response=request(&router,"/s/123/mail",json!({"action":"submit","requestID":uuid::Uuid::new_v4(),"emailId":"draft","identityId":"sender"}),cookie,"https://studio.webdock.test").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Value>(&response.into_body().collect().await.unwrap().to_bytes())
            .unwrap(),
        json!({"data":{"id":"789","state":"queued","filed":false}})
    );
    assert_eq!(control.submissions.load(Ordering::SeqCst), 1);
    control.revoked.store(true, Ordering::SeqCst);
    assert_eq!(
        request(
            &router,
            "/s/123/mail",
            json!({"action":"folders"}),
            cookie,
            "https://studio.webdock.test"
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    task.abort();
}

#[tokio::test]
async fn fixture_transport_override_cannot_authorize_a_different_runtime_binding() {
    let control = Arc::new(FixtureControl {
        revoked: AtomicBool::new(false),
        runtime: "http://127.0.0.1:1".into(),
        submissions: AtomicUsize::new(0),
    });
    let mut options =
        GatewayOptions::new(vec!["https://studio.webdock.test".into()], control).unwrap();
    options.test_runtime_origin = Some("http://127.0.0.1:1".into());
    let router = Gateway::new(options).unwrap().router();
    let launch = request(
        &router,
        "/session",
        json!({"code":"x".repeat(43)}),
        "",
        "https://studio.webdock.test",
    )
    .await;
    assert_eq!(launch.status(), StatusCode::OK);
    let cookie = launch.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    assert_eq!(
        request(
            &router,
            "/s/123/mail",
            json!({"action":"folders"}),
            cookie,
            "https://studio.webdock.test",
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED,
        "Canonical assigned runtime binding must be checked before any fixture transport attempt"
    );
}

struct JournalControl {
    base: FixtureControl,
    reserved: AtomicBool,
    finished: AtomicBool,
}
#[async_trait::async_trait]
impl Control for JournalControl {
    async fn call(&self, input: Value) -> Result<Value, ControlFailure> {
        match input["action"].as_str().unwrap() {
            "reserve" => {
                let execute = !self.reserved.swap(true, Ordering::SeqCst);
                Ok(
                    json!({"execute":execute,"operationID":"456","leaseToken":"11111111-1111-4111-8111-111111111111","state":if execute{"reserved"}else{"uncertain"},"result":null}),
                )
            }
            "finish" => {
                assert_eq!(input["write"]["state"], "uncertain");
                self.finished.store(true, Ordering::SeqCst);
                Ok(json!({"ok":true}))
            }
            _ => self.base.call(input).await,
        }
    }
    async fn send(&self, _input: Value) -> Result<Value, ControlFailure> {
        panic!("update does not send mail")
    }
}
#[tokio::test]
async fn uncertain_write_is_reserved_before_network_and_is_never_attempted_twice() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let runtime = format!("http://{}", listener.local_addr().unwrap());
    let control = Arc::new(JournalControl {
        base: FixtureControl {
            revoked: AtomicBool::new(false),
            runtime: "http://app-12.wd-mail-12.svc.cluster.local:8080".into(),
            submissions: AtomicUsize::new(0),
        },
        reserved: AtomicBool::new(false),
        finished: AtomicBool::new(false),
    });
    let writes = Arc::new(AtomicUsize::new(0));
    let calls = writes.clone();
    let admission = control.clone();
    let upstream = Router::new().route(
        "/jmap",
        post(move |Json(body): Json<Value>| {
            let calls = calls.clone();
            let admission = admission.clone();
            async move {
                assert!(admission.reserved.load(Ordering::SeqCst));
                assert_eq!(body["methodCalls"][0][0], "Email/set");
                calls.fetch_add(1, Ordering::SeqCst);
                (StatusCode::BAD_GATEWAY, "PRIVATE upstream outcome")
            }
        }),
    );
    let task = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let mut options =
        GatewayOptions::new(vec!["https://studio.webdock.test".into()], control.clone()).unwrap();
    options.test_runtime_origin = Some(runtime);
    let router = Gateway::new(options).unwrap().router();
    let launch = request(
        &router,
        "/session",
        json!({"code":"x".repeat(43)}),
        "",
        "https://studio.webdock.test",
    )
    .await;
    let cookie = launch.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let command = json!({"action":"update","requestID":uuid::Uuid::new_v4(),"emailId":"email1","change":{"seen":true}});
    let first = request(
        &router,
        "/s/123/mail",
        command.clone(),
        &cookie,
        "https://studio.webdock.test",
    )
    .await;
    assert_eq!(first.status(), StatusCode::BAD_GATEWAY);
    let body = first.into_body().collect().await.unwrap().to_bytes();
    assert!(!String::from_utf8_lossy(&body).contains("PRIVATE"));
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["uncertain"],
        true
    );
    assert!(control.finished.load(Ordering::SeqCst));
    let second = request(
        &router,
        "/s/123/mail",
        command,
        &cookie,
        "https://studio.webdock.test",
    )
    .await;
    assert_eq!(second.status(), StatusCode::CONFLICT);
    assert_eq!(writes.load(Ordering::SeqCst), 1);
    task.abort();
}

#[tokio::test]
async fn revocation_during_upstream_read_discards_content() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let runtime = format!("http://{}", listener.local_addr().unwrap());
    let control = Arc::new(FixtureControl {
        revoked: AtomicBool::new(false),
        runtime: "http://app-12.wd-mail-12.svc.cluster.local:8080".into(),
        submissions: AtomicUsize::new(0),
    });
    let admission = control.clone();
    let upstream=Router::new().route("/jmap",post(move |Json(body):Json<Value>|{let admission=admission.clone();async move{admission.revoked.store(true,Ordering::SeqCst);let call=&body["methodCalls"][0];Json(json!({"methodResponses":[[call[0],{"accountId":"account-a","list":[{"id":"inbox","name":"PRIVATE revoked content"}]},call[2]]]}))}}));
    let task = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let mut options =
        GatewayOptions::new(vec!["https://studio.webdock.test".into()], control).unwrap();
    options.test_runtime_origin = Some(runtime);
    let router = Gateway::new(options).unwrap().router();
    let launch = request(
        &router,
        "/session",
        json!({"code":"x".repeat(43)}),
        "",
        "https://studio.webdock.test",
    )
    .await;
    let cookie = launch.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let response = request(
        &router,
        "/s/123/mail",
        json!({"action":"folders"}),
        &cookie,
        "https://studio.webdock.test",
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        !String::from_utf8_lossy(&response.into_body().collect().await.unwrap().to_bytes())
            .contains("PRIVATE")
    );
    task.abort();
}
