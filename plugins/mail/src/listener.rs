//! Generic RPC and native HTTP share one listener and mutually authenticated connection.
use axum::{
    Router,
    extract::Request,
    http::StatusCode,
    middleware::{self, Next},
    response::Response,
};
use std::{future::Future, sync::Arc, time::Duration};
use webdock_plugin_runtime::{CorePeer, MutualTls, Runtime, UnixSocket};

pub fn native_routes(router: Router, peers: Vec<CorePeer>) -> Router {
    let peers = Arc::new(peers);
    router.layer(middleware::from_fn(move |request: Request, next: Next| {
        let peers = peers.clone();
        async move {
            let mut probe = tonic::Request::new(());
            *probe.extensions_mut() = request.extensions().clone();
            let allowed = webdock_plugin_runtime::authenticated_peer(&probe)
                .is_ok_and(|peer| peers.iter().any(|p| p.permits(&peer)));
            if !allowed {
                return Response::builder()
                    .status(StatusCode::FORBIDDEN)
                    .header("cache-control", "no-store")
                    .header("x-content-type-options", "nosniff")
                    .header("connection", "close")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        r#"{"error":"Mail request could not be completed."}"#,
                    ))
                    .expect("static denial response");
            }
            next.run(request).await
        }
    }))
}
pub async fn serve_mtls(
    runtime: Runtime,
    router: Router,
    listener: tokio::net::TcpListener,
    tls: MutualTls,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), tonic::transport::Error> {
    let routes = tonic::service::Routes::from(router).add_service(runtime.service());
    tonic::transport::Server::builder()
        .tls_config(
            tonic::transport::ServerTlsConfig::new()
                .identity(tls.identity)
                .client_ca_root(tls.peer_ca)
                .client_auth_optional(false)
                .timeout(Duration::from_secs(5)),
        )?
        .timeout(Duration::from_secs(55))
        .concurrency_limit_per_connection(32)
        .add_routes(routes)
        .serve_with_incoming_shutdown(
            tokio_stream::wrappers::TcpListenerStream::new(listener),
            shutdown,
        )
        .await
}
pub async fn serve_unix(
    runtime: Runtime,
    router: Router,
    mut socket: UnixSocket,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), tonic::transport::Error> {
    let routes = tonic::service::Routes::from(router).add_service(runtime.service());
    let incoming = socket.incoming().expect("owned socket");
    let result = tonic::transport::Server::builder()
        .timeout(Duration::from_secs(55))
        .concurrency_limit_per_connection(32)
        .add_routes(routes)
        .serve_with_incoming_shutdown(incoming, shutdown)
        .await;
    drop(socket);
    result
}
