//! Quarantine-only TLS inspection of an actual restored CE/ntfy process.
//! This gateway cannot submit Mail, resume a queue or mutate application state.
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use serde::Deserialize;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::net::TcpListener;
use tokio_rustls::{TlsAcceptor, rustls};
use webdock_supervisor::{ExecutionError, Result};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    Mail,
    Ntfy,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Configuration {
    pub profile: Profile,
    pub certificate: PathBuf,
    pub private_key: PathBuf,
    pub port: u16,
    pub upstream_port: u16,
    pub deadline_seconds: u64,
}
struct Context {
    profile: Profile,
    upstream: String,
    http: reqwest::Client,
}
fn deny() -> Response {
    (StatusCode::FORBIDDEN, "Recovery target is read-only").into_response()
}
fn unavailable() -> Response {
    (StatusCode::BAD_GATEWAY, "Recovery inspection unavailable").into_response()
}
fn read_methods(bytes: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return false;
    };
    value["methodCalls"].as_array().is_some_and(|calls| {
        !calls.is_empty()
            && calls.len() <= 20
            && calls.iter().all(|call| {
                call.as_array().is_some_and(|v| {
                    v.len() == 3
                        && v[0].as_str().is_some_and(|method| {
                            [
                                "Core/echo",
                                "Mailbox/get",
                                "Email/get",
                                "Email/query",
                                "Thread/get",
                            ]
                            .contains(&method)
                        })
                        && v[1].is_object()
                        && v[2].is_string()
                })
            })
    })
}
async fn proxy(State(context): State<Arc<Context>>, request: Request) -> Response {
    let path = request.uri().path();
    let allowed = match context.profile {
        Profile::Mail => {
            (request.method() == Method::GET && path == "/jmap/session")
                || (request.method() == Method::POST && path == "/jmap")
        }
        Profile::Ntfy => {
            request.method() == Method::GET
                && (path == "/v1/health" || path.ends_with("/json") || path.starts_with("/file/"))
        }
    };
    if !allowed {
        return deny();
    }
    let authorization = request.headers().get(header::AUTHORIZATION).cloned();
    let method = request.method().clone();
    let uri = request
        .uri()
        .path_and_query()
        .map(|v| v.as_str())
        .unwrap_or("/")
        .to_owned();
    let Ok(body) = to_bytes(request.into_body(), 131072).await else {
        return deny();
    };
    if matches!(context.profile, Profile::Mail) && method == Method::POST && !read_methods(&body) {
        return deny();
    }
    // Stalwart may publish an anonymous Session document. Quarantine inspection
    // requires credentials even for that metadata; Stalwart still validates them.
    if matches!(context.profile, Profile::Mail)
        && authorization
            .as_ref()
            .is_none_or(|value| value.as_bytes().is_empty())
    {
        return (
            StatusCode::UNAUTHORIZED,
            "Recovery inspection authentication required",
        )
            .into_response();
    }
    let mut outgoing = context
        .http
        .request(method, format!("{}{}", context.upstream, uri));
    if let Some(auth) = authorization {
        outgoing = outgoing.header(header::AUTHORIZATION, auth);
    }
    if !body.is_empty() {
        outgoing = outgoing
            .header(header::CONTENT_TYPE, "application/json")
            .body(body);
    }
    let Ok(response) = outgoing.send().await else {
        return unavailable();
    };
    let status = response.status();
    let content_type = response.headers().get(header::CONTENT_TYPE).cloned();
    if response
        .content_length()
        .is_some_and(|n| n > 32 * 1024 * 1024)
    {
        return unavailable();
    }
    let mut response = response;
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if bytes.len() + chunk.len() > 32 * 1024 * 1024 {
                    return unavailable();
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(_) => return unavailable(),
        }
    }
    let mut output = Response::new(Body::from(bytes));
    *output.status_mut() = status;
    if let Some(content_type) = content_type {
        output
            .headers_mut()
            .insert(header::CONTENT_TYPE, content_type);
    }
    output
}
pub async fn serve(configuration: Configuration) -> Result<()> {
    if configuration.port < 1024
        || configuration.upstream_port < 1024
        || !(1..=900).contains(&configuration.deadline_seconds)
    {
        return Err(ExecutionError::Invalid("recovery inspection listener"));
    }
    let certificate = crate::config::read_private(&configuration.certificate, 65536)
        .map_err(|_| ExecutionError::Ownership)?;
    let key = crate::config::read_private(&configuration.private_key, 65536)
        .map_err(|_| ExecutionError::Ownership)?;
    let chain = rustls_pemfile::certs(&mut certificate.as_slice())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| ExecutionError::Invalid("recovery inspection certificate"))?;
    let key = rustls_pemfile::private_key(&mut key.as_slice())
        .map_err(|_| ExecutionError::Invalid("recovery inspection key"))?
        .ok_or(ExecutionError::Invalid("recovery inspection key"))?;
    let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| ExecutionError::Runtime)?
    .with_no_client_auth()
    .with_single_cert(chain, key)
    .map_err(|_| ExecutionError::Invalid("recovery inspection TLS"))?;
    let context = Arc::new(Context {
        profile: configuration.profile,
        upstream: format!("http://127.0.0.1:{}", configuration.upstream_port),
        http: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| ExecutionError::Runtime)?,
    });
    let app = Router::new().fallback(any(proxy)).with_state(context);
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, configuration.port))
        .await
        .map_err(|_| ExecutionError::Runtime)?;
    let acceptor = TlsAcceptor::from(Arc::new(tls));
    let deadline = tokio::time::sleep(Duration::from_secs(configuration.deadline_seconds));
    tokio::pin!(deadline);
    let mut clients = tokio::task::JoinSet::new();
    let limit = Arc::new(tokio::sync::Semaphore::new(8));
    loop {
        tokio::select! {
            ()=&mut deadline=>break,
            accepted=listener.accept()=>{
                let (stream,_)=accepted.map_err(|_|ExecutionError::Runtime)?;
                let Ok(permit)=limit.clone().try_acquire_owned()else{continue};let acceptor=acceptor.clone();let app=app.clone();
                clients.spawn(async move {
                    let _permit=permit;
                    if let Ok(Ok(tls))=tokio::time::timeout(Duration::from_secs(5),acceptor.accept(stream)).await {
                        let _=tokio::time::timeout(Duration::from_secs(30),hyper::server::conn::http1::Builder::new().keep_alive(false).serve_connection(hyper_util::rt::TokioIo::new(tls),hyper_util::service::TowerToHyperService::new(app))).await;
                    }
                });
            },
            _=clients.join_next(),if !clients.is_empty()=>{},
        }
    }
    clients.abort_all();
    while clients.join_next().await.is_some() {}
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn mailbox_session_without_authorization_is_denied_before_upstream_contact() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().fallback(move || {
            let calls = observed.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                axum::Json(serde_json::json!({"accounts":{},"primaryAccounts":{}}))
            }
        });
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let context = Arc::new(Context {
            profile: Profile::Mail,
            upstream: format!("http://{address}"),
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
        });
        let request = Request::builder()
            .uri("/jmap/session")
            .body(Body::empty())
            .unwrap();
        let response = proxy(State(context), request).await;
        server.abort();
        let _ = server.await;
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "a public upstream session must not bypass quarantine authentication"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn mailbox_content_inspection_cannot_mutate_or_resume_a_historical_queue() {
        assert!(read_methods(
            br#"{"methodCalls":[["Email/get",{"accountId":"fixture","ids":["one"]},"r"]]}"#
        ));
        for method in [
            "Email/set",
            "EmailSubmission/set",
            "x:Action/set",
            "x:Account/get",
            "Mailbox/set",
        ] {
            assert!(!read_methods(
                &serde_json::to_vec(&serde_json::json!({"methodCalls":[[method,{},"r"]]})).unwrap()
            ));
        }
    }
}
