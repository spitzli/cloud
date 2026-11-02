use axum::Router;
use futures_util::FutureExt;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
pub struct TlsServer {
    pub origin: String,
    pub ca: String,
    task: tokio::task::JoinHandle<()>,
    panicked: Arc<AtomicBool>,
}
impl Drop for TlsServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl TlsServer {
    pub fn assert_healthy(&self) {
        assert!(
            !self.task.is_finished(),
            "TLS listener unexpectedly stopped"
        );
        assert!(
            !self.panicked.load(Ordering::SeqCst),
            "HTTP fixture task panicked"
        );
    }
    pub async fn bind(router: impl FnOnce(&str) -> Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = Self::origin(&listener);
        Self::serve(listener, router(&origin))
    }
    /// HTTPS origin a pre-bound loopback listener will serve, so peers can be configured first.
    pub fn origin(listener: &tokio::net::TcpListener) -> String {
        format!(
            "https://localhost:{}",
            listener.local_addr().unwrap().port()
        )
    }
    pub fn serve(listener: tokio::net::TcpListener, router: Router) -> Self {
        let certificate = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let ca = certificate.cert.pem();
        let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![certificate.cert.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(certificate.signing_key.serialize_der())
                .into(),
        )
        .unwrap();
        let origin = format!("{}/", Self::origin(&listener));
        let panicked = Arc::new(AtomicBool::new(false));
        let failure = panicked.clone();
        let task = tokio::spawn(async move {
            let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted=listener.accept()=>{
                        let (socket,_)=accepted.unwrap();let acceptor=acceptor.clone();let service=router.clone();let panicked=failure.clone();
                        connections.spawn(async move {
                            let result=std::panic::AssertUnwindSafe(async move {
                            let Ok(stream)=acceptor.accept(socket).await else{return;};
                            let io=hyper_util::rt::TokioIo::new(stream);
                            let service=hyper_util::service::TowerToHyperService::new(service);
                            let _=hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new()).serve_connection(io,service).await;
                            }).catch_unwind().await;
                            if result.is_err(){panicked.store(true,Ordering::SeqCst);}
                        });
                    },
                    result=connections.join_next(),if !connections.is_empty()=>{if result.is_some_and(|r|r.is_err()){failure.store(true,Ordering::SeqCst);}}
                }
            }
        });
        Self {
            origin,
            ca,
            task,
            panicked,
        }
    }
}
