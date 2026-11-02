use axum::{
    Router,
    body::{Body, to_bytes},
    extract::Request,
    http::{Response, StatusCode},
    routing::any,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub struct FaultProxy {
    pub endpoint: String,
    pub lost: Arc<AtomicBool>,
    pub reject_complete: Arc<AtomicBool>,
    pub truncate_reads: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}
impl FaultProxy {
    pub async fn start(upstream: String) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let lost = Arc::new(AtomicBool::new(false));
        let state = lost.clone();
        let reject_complete = Arc::new(AtomicBool::new(false));
        let reject = reject_complete.clone();
        let truncate_reads = Arc::new(AtomicBool::new(false));
        let truncate = truncate_reads.clone();
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let router = Router::new().route(
            "/{*path}",
            any(move |request: Request| {
                let client = client.clone();
                let upstream = upstream.clone();
                let lost = state.clone();
                let reject = reject.clone();
                let truncate = truncate.clone();
                async move {
                    let (parts, body) = request.into_parts();
                    let complete = parts.method == "POST"
                        && parts.uri.query().is_some_and(|q| q.contains("uploadId="));
                    if complete && reject.load(Ordering::SeqCst) {
                        return Response::builder()
                            .status(StatusCode::FORBIDDEN)
                            .body(Body::empty())
                            .unwrap();
                    }
                    let truncated = parts.method == "GET" && truncate.load(Ordering::SeqCst);
                    let body = to_bytes(body, 32 * 1024 * 1024).await.unwrap();
                    let response = client
                        .request(parts.method, format!("{upstream}{}", parts.uri))
                        .headers(parts.headers)
                        .body(body)
                        .send()
                        .await
                        .unwrap();
                    let status = response.status();
                    let headers = response.headers().clone();
                    let bytes = response.bytes().await.unwrap();
                    if complete && status.is_success() && !lost.swap(true, Ordering::SeqCst) {
                        return Response::builder()
                            .status(StatusCode::SERVICE_UNAVAILABLE)
                            .body(Body::empty())
                            .unwrap();
                    }
                    let body = if truncated && !bytes.is_empty() {
                        Body::from_stream(futures_util::stream::iter([Ok::<_, std::io::Error>(
                            bytes.slice(..1),
                        )]))
                    } else {
                        Body::from(bytes)
                    };
                    let mut response = Response::builder().status(status).body(body).unwrap();
                    *response.headers_mut() = headers;
                    response
                }
            }),
        );
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            endpoint,
            lost,
            reject_complete,
            truncate_reads,
            task,
        }
    }
}
impl Drop for FaultProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}
