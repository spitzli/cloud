use std::{
    os::unix::fs::MetadataExt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};
use tonic::{Request, Response, Status};
use webdock_plugin_runtime::{HostClient, UnixSocket, wire};

#[derive(Clone)]
struct Host {
    mode: Arc<AtomicUsize>,
    attempts: mpsc::UnboundedSender<()>,
    release: Arc<tokio::sync::Notify>,
}
#[tonic::async_trait]
impl wire::plugin_host_server::PluginHost for Host {
    async fn authorize_peer(
        &self,
        request: Request<wire::HostPeerRequest>,
    ) -> Result<Response<wire::HostAuthorityResponse>, Status> {
        let request = request.into_inner();
        assert_eq!(
            (request.plugin_id.as_str(), request.generation),
            ("mail", 2)
        );
        let mode = self.mode.load(Ordering::SeqCst);
        let _ = self.attempts.send(());
        match mode {
            0 => Ok(Response::new(wire::HostAuthorityResponse { active: false })),
            1 => Ok(Response::new(wire::HostAuthorityResponse { active: true })),
            2 => Err(Status::unavailable("fixture outage")),
            _ => {
                self.release.notified().await;
                Ok(Response::new(wire::HostAuthorityResponse {
                    active: self.mode.load(Ordering::SeqCst) == 1,
                }))
            }
        }
    }
    async fn authorize_invocation(
        &self,
        _: Request<wire::HostInvocationRequest>,
    ) -> Result<Response<wire::HostAuthorityResponse>, Status> {
        Err(Status::permission_denied("unused"))
    }
    async fn authorize_request(
        &self,
        _: Request<wire::HostPayloadRequest>,
    ) -> Result<Response<wire::HostAuthorityResponse>, Status> {
        Err(Status::permission_denied("unused"))
    }
    async fn call(
        &self,
        _: Request<wire::HostCallRequest>,
    ) -> Result<Response<wire::HostCallResponse>, Status> {
        Err(Status::permission_denied("unused"))
    }
    async fn workloads(
        &self,
        _: Request<wire::HostWorkloadRequest>,
    ) -> Result<Response<wire::HostCallResponse>, Status> {
        Err(Status::permission_denied("unused"))
    }
}
struct Fixture {
    task: tokio::task::JoinHandle<()>,
    directory: PathBuf,
    client: HostClient,
    host: Host,
    attempts: mpsc::UnboundedReceiver<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
impl Fixture {
    async fn new() -> Self {
        use std::os::unix::fs::DirBuilderExt;
        let directory =
            std::env::temp_dir().join(format!("mail-startup-{}", uuid::Uuid::new_v4().simple()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let path = directory.join("core.sock");
        let mut socket = UnixSocket::bind(&path).unwrap();
        let incoming = socket.incoming().unwrap();
        let (attempts, receiver) = mpsc::unbounded_channel();
        let host = Host {
            mode: Arc::default(),
            attempts,
            release: Arc::default(),
        };
        let service = wire::plugin_host_server::PluginHostServer::new(host.clone());
        let task = tokio::spawn(async move {
            let _socket = socket;
            tonic::transport::Server::builder()
                .add_service(service)
                .serve_with_incoming(incoming)
                .await
                .unwrap();
        });
        let client = HostClient::connect_unix(path).await.unwrap();
        Self {
            task,
            directory,
            client,
            host,
            attempts: receiver,
        }
    }
    async fn attempt(&mut self) {
        tokio::time::timeout(Duration::from_secs(3), self.attempts.recv())
            .await
            .unwrap()
            .unwrap();
    }
}

async fn serve_until_listener_exit<S, F, R, E>(
    server: S,
    client: HostClient,
    plugin: &str,
    generation: u64,
    resume: F,
) -> Result<(), crate::Error>
where
    S: std::future::Future<Output = Result<(), crate::Error>>,
    F: FnMut() -> R,
    R: std::future::Future<Output = Result<(), E>>,
{
    super::serve(
        |_| server,
        std::future::pending(),
        client,
        plugin,
        generation,
        resume,
    )
    .await
}

#[tokio::test]
async fn candidate_and_outage_have_no_resume_effects_then_promotion_resumes_automatically() {
    let mut f = Fixture::new().await;
    let serving = Arc::new(AtomicBool::new(false));
    let ready = serving.clone();
    let (stop, stopped) = oneshot::channel();
    let server = async move {
        ready.store(true, Ordering::SeqCst);
        let _ = stopped.await;
        Ok(())
    };
    let (resumed, mut effects) = mpsc::unbounded_channel();
    let task = tokio::spawn(serve_until_listener_exit(
        server,
        f.client.clone(),
        "mail",
        2,
        move || {
            assert!(
                serving.load(Ordering::SeqCst),
                "listener must be polled first"
            );
            resumed.send(()).unwrap();
            std::future::ready(Ok::<(), ()>(()))
        },
    ));
    f.attempt().await;
    assert!(effects.try_recv().is_err());
    f.host.mode.store(2, Ordering::SeqCst);
    f.attempt().await;
    assert!(effects.try_recv().is_err());
    f.host.mode.store(1, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(3), effects.recv())
        .await
        .unwrap()
        .unwrap();
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
    assert!(effects.try_recv().is_err());
}

#[tokio::test]
async fn failed_resume_requires_fresh_authority_before_retrying() {
    let mut f = Fixture::new().await;
    f.host.mode.store(1, Ordering::SeqCst);
    let mode = f.host.mode.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let (resumed, mut effects) = mpsc::unbounded_channel();
    let (stop, stopped) = oneshot::channel();
    let server = async {
        let _ = stopped.await;
        Ok(())
    };
    let task = tokio::spawn(serve_until_listener_exit(
        server,
        f.client.clone(),
        "mail",
        2,
        move || {
            let first = count.fetch_add(1, Ordering::SeqCst) == 0;
            if first {
                mode.store(0, Ordering::SeqCst);
            }
            resumed.send(()).unwrap();
            std::future::ready(if first { Err(()) } else { Ok(()) })
        },
    ));
    tokio::time::timeout(Duration::from_secs(3), effects.recv())
        .await
        .unwrap()
        .unwrap();
    f.attempt().await;
    f.attempt().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    f.host.mode.store(1, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(3), effects.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn listener_shutdown_cancels_an_inflight_authorization_without_resume() {
    let mut f = Fixture::new().await;
    f.host.mode.store(3, Ordering::SeqCst);
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let (stop, stopped) = oneshot::channel();
    let server = async {
        let _ = stopped.await;
        Ok(())
    };
    let task = tokio::spawn(serve_until_listener_exit(
        server,
        f.client.clone(),
        "mail",
        2,
        move || {
            count.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Ok::<(), ()>(()))
        },
    ));
    f.attempt().await;
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn listener_failure_cancels_backoff_without_later_resume() {
    let mut f = Fixture::new().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let (stop, stopped) = oneshot::channel();
    let server = async {
        let _ = stopped.await;
        Err("fixture listener failed".into())
    };
    let task = tokio::spawn(serve_until_listener_exit(
        server,
        f.client.clone(),
        "mail",
        2,
        move || {
            count.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Ok::<(), ()>(()))
        },
    ));
    f.attempt().await;
    // The RPC has replied inactive; let the client enter its backoff wait.
    tokio::time::sleep(Duration::from_millis(20)).await;
    stop.send(()).unwrap();
    let error = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.to_string(), "fixture listener failed");
    f.host.mode.store(1, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(
        f.attempts.try_recv().is_err(),
        "cancelled startup must not keep polling"
    );
}

#[tokio::test]
async fn listener_shutdown_drops_an_inflight_resume_future() {
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let f = Fixture::new().await;
    f.host.mode.store(1, Ordering::SeqCst);
    let dropped = Arc::new(AtomicBool::new(false));
    let flag = dropped.clone();
    let (entered, mut resumes) = mpsc::unbounded_channel();
    let (stop, stopped) = oneshot::channel();
    let server = async {
        let _ = stopped.await;
        Ok(())
    };
    let task = tokio::spawn(serve_until_listener_exit(
        server,
        f.client.clone(),
        "mail",
        2,
        move || {
            let guard = Dropped(flag.clone());
            entered.send(()).unwrap();
            async move {
                let _guard = guard;
                std::future::pending::<Result<(), ()>>().await
            }
        },
    ));
    tokio::time::timeout(Duration::from_secs(3), resumes.recv())
        .await
        .unwrap()
        .unwrap();
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn shutdown_prevents_resume_when_callback_grants_during_real_graceful_drain() {
    let mut f = Fixture::new().await;
    f.host.mode.store(3, Ordering::SeqCst);
    let (attempts, mut drain_attempts) = mpsc::unbounded_channel();
    let drain_host = Host {
        mode: Arc::new(AtomicUsize::new(3)),
        attempts,
        release: Arc::default(),
    };
    let path = f.directory.join("drain.sock");
    let mut socket = UnixSocket::bind(&path).unwrap();
    let incoming = socket.incoming().unwrap();
    let service = wire::plugin_host_server::PluginHostServer::new(drain_host.clone());
    let (signal, shutdown) = oneshot::channel();
    let (draining, drain_started) = oneshot::channel();
    let server = move |stopped: oneshot::Receiver<()>| async move {
        let _socket = socket;
        tonic::transport::Server::builder()
            .add_service(service)
            .serve_with_incoming_shutdown(incoming, async {
                let _ = stopped.await;
                let _ = draining.send(());
            })
            .await?;
        Ok(())
    };
    let (resumed, mut effects) = mpsc::unbounded_channel();
    let task = tokio::spawn(super::serve(
        server,
        async {
            let _ = shutdown.await;
        },
        f.client.clone(),
        "mail",
        2,
        move || {
            let _ = resumed.send(());
            std::future::ready(Ok::<(), ()>(()))
        },
    ));
    f.attempt().await;
    let mut draining_client = wire::plugin_host_client::PluginHostClient::new(
        webdock_plugin_runtime::unix_channel(path).await.unwrap(),
    );
    let request = tokio::spawn(async move {
        draining_client
            .authorize_peer(wire::HostPeerRequest {
                plugin_id: "mail".into(),
                generation: 2,
            })
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), drain_attempts.recv())
        .await
        .unwrap()
        .unwrap();
    signal.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), drain_started)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !task.is_finished(),
        "real pending RPC must hold graceful drain open"
    );
    f.host.mode.store(1, Ordering::SeqCst);
    f.host.release.notify_waiters();
    let resumed_after_shutdown = matches!(
        tokio::time::timeout(Duration::from_millis(300), effects.recv()).await,
        Ok(Some(()))
    );
    assert!(
        !task.is_finished(),
        "drain remains blocked independently of callback grant"
    );
    drain_host.release.notify_waiters();
    request.await.unwrap().unwrap();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        !resumed_after_shutdown,
        "shutdown must fence resume before graceful drain completes"
    );
}

#[tokio::test]
async fn initially_offline_callback_preserves_discovery_then_resumes_when_available() {
    struct NoActions;
    #[tonic::async_trait]
    impl webdock_plugin_runtime::ActionHandler for NoActions {
        async fn invoke(
            &self,
            _: &webdock_plugin_protocol::InvocationClaims,
            _: &str,
            _: serde_json::Value,
            _: serde_json::Value,
        ) -> Result<serde_json::Value, webdock_plugin_protocol::PluginError> {
            panic!("discovery must not invoke a product action");
        }
    }
    let mut f = Fixture::new().await;
    f.task.abort();
    let _ = (&mut f.task).await;
    let core_path = f.directory.join("core.sock");
    assert!(!core_path.exists());
    let path = f.directory.join("plugin.sock");
    let transport = webdock_plugin_kit::transport::RuntimeTransport {
        listen: Some(path.clone()),
        core: Some(core_path.clone()),
        ..Default::default()
    };
    let client = transport.core_client_lazy().await.unwrap();
    let authority = webdock_plugin_runtime::RemoteAuthority::new(
        client.clone(),
        "mail".into(),
        2,
        vec![webdock_plugin_runtime::CorePeer::Unix {
            uid: std::fs::metadata(&f.directory).unwrap().uid(),
        }],
    )
    .unwrap();
    let runtime = webdock_plugin_runtime::Runtime::new(
        webdock_plugin_protocol::Manifest::parse(include_bytes!("../manifest.json")).unwrap(),
        2,
        ed25519_dalek::SigningKey::from_bytes(&[71; 32]).verifying_key(),
        Arc::new(authority),
        Arc::new(NoActions),
    )
    .unwrap();
    let socket = UnixSocket::bind(&path).unwrap();
    let (stop, stopped) = oneshot::channel();
    let (resumed, mut effects) = mpsc::unbounded_channel();
    let task = tokio::spawn(super::serve(
        |drain| async {
            Ok(runtime
                .serve_unix(socket, async {
                    let _ = drain.await;
                })
                .await?)
        },
        async {
            let _ = stopped.await;
        },
        client,
        "mail",
        2,
        move || {
            let _ = resumed.send(());
            std::future::ready(Ok::<(), ()>(()))
        },
    ));
    let mut probe = webdock_plugin_runtime::connect_unix(path).await.unwrap();
    let health = probe
        .health(wire::HealthRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!((health.plugin_id.as_str(), health.generation), ("mail", 2));
    assert!(
        !probe
            .describe(wire::DescribeRequest {})
            .await
            .unwrap()
            .into_inner()
            .manifest_json
            .is_empty()
    );
    assert!(effects.try_recv().is_err());
    let mut socket = UnixSocket::bind(core_path).unwrap();
    let incoming = socket.incoming().unwrap();
    f.host.mode.store(1, Ordering::SeqCst);
    let service = wire::plugin_host_server::PluginHostServer::new(f.host.clone());
    f.task = tokio::spawn(async move {
        let _socket = socket;
        tonic::transport::Server::builder()
            .add_service(service)
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    tokio::time::timeout(Duration::from_secs(3), effects.recv())
        .await
        .unwrap()
        .unwrap();
    drop(probe);
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn shutdown_signal_remains_polled_after_resume_completes() {
    let f = Fixture::new().await;
    f.host.mode.store(1, Ordering::SeqCst);
    let (signal, shutdown) = oneshot::channel();
    let (draining, drain_started) = oneshot::channel();
    let (release, drain_finished) = oneshot::channel();
    let (resumed, mut effects) = mpsc::unbounded_channel();
    let task = tokio::spawn(super::serve(
        |drain| async {
            let _ = drain.await;
            let _ = draining.send(());
            let _ = drain_finished.await;
            Ok(())
        },
        async {
            let _ = shutdown.await;
        },
        f.client.clone(),
        "mail",
        2,
        move || {
            let _ = resumed.send(());
            std::future::ready(Ok::<(), ()>(()))
        },
    ));
    tokio::time::timeout(Duration::from_secs(1), effects.recv())
        .await
        .unwrap()
        .unwrap();
    signal.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), drain_started)
        .await
        .unwrap()
        .unwrap();
    assert!(!task.is_finished());
    release.send(()).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn direct_shutdown_drops_resume_before_server_drain_finishes() {
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let f = Fixture::new().await;
    f.host.mode.store(1, Ordering::SeqCst);
    let dropped = Arc::new(AtomicBool::new(false));
    let flag = dropped.clone();
    let (signal, shutdown) = oneshot::channel();
    let (draining, drain_started) = oneshot::channel();
    let (release, drain_finished) = oneshot::channel();
    let (entered, mut resumes) = mpsc::unbounded_channel();
    let task = tokio::spawn(super::serve(
        |drain| async {
            let _ = drain.await;
            let _ = draining.send(());
            let _ = drain_finished.await;
            Ok(())
        },
        async {
            let _ = shutdown.await;
        },
        f.client.clone(),
        "mail",
        2,
        move || {
            let guard = Dropped(flag.clone());
            let _ = entered.send(());
            async move {
                let _guard = guard;
                std::future::pending::<Result<(), ()>>().await
            }
        },
    ));
    tokio::time::timeout(Duration::from_secs(1), resumes.recv())
        .await
        .unwrap()
        .unwrap();
    signal.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), drain_started)
        .await
        .unwrap()
        .unwrap();
    assert!(dropped.load(Ordering::SeqCst));
    assert!(
        !task.is_finished(),
        "resume cancellation must not wait for drain completion"
    );
    release.send(()).unwrap();
    task.await.unwrap().unwrap();
}

// Local listener lifetime calibration only: no real Core installation, metadata DB or provider.
// In particular this does not qualify the concrete ManagedTransport or Core RuntimeConnection.
#[test]
fn lifetime_diagnostics_are_exact_flag_static_enums_only() {
    use super::LifetimeEvent::*;
    let events = [
        Sigterm,
        Sigint,
        SignalApiError,
        SignalStreamClosed,
        ServerOk,
        ServerErr,
        RequestedShutdown,
        DispatcherStopped,
    ];
    let tokens = [
        "SIGTERM",
        "SIGINT",
        "signal_api_error",
        "signal_stream_closed",
        "server_ok",
        "server_err",
        "requested_shutdown",
        "dispatcher_stopped",
    ];
    for (event, token) in events.into_iter().zip(tokens) {
        for flag in [
            None,
            Some("0"),
            Some("true"),
            Some("01"),
            Some("private-poison"),
        ] {
            assert!(event.line(flag.map(std::ffi::OsStr::new)).is_none());
        }
        assert_eq!(
            event.line(Some(std::ffi::OsStr::new("1"))),
            Some(format!("mail_lifetime event={token}\n"))
        );
    }
}
struct LifetimeDeniedControl(AtomicUsize);
#[tonic::async_trait]
impl webdock_mail_runtime::SendControl for LifetimeDeniedControl {
    async fn call(
        &self,
        _: serde_json::Value,
    ) -> Result<serde_json::Value, webdock_mail_runtime::ControlError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(webdock_mail_runtime::ControlError { status: 403 })
    }
}
struct LifetimeDeniedPayloads;
#[tonic::async_trait]
impl webdock_mail_runtime::PrivatePayloadStore for LifetimeDeniedPayloads {
    async fn put(
        &self,
        _: &str,
        _: &str,
        _: &[u8],
    ) -> webdock_mail_runtime::Result<webdock_mail_runtime::SavedPayload> {
        panic!("lifetime calibration must never write a payload")
    }
    async fn get(&self, _: &str, _: &str) -> webdock_mail_runtime::Result<Vec<u8>> {
        panic!("lifetime calibration must never read a payload")
    }
    async fn remove(&self, _: &str, _: &str) -> webdock_mail_runtime::Result<()> {
        panic!("lifetime calibration must never remove a payload")
    }
}
struct LifetimeDeniedTransport;
#[tonic::async_trait]
impl webdock_mail_runtime::dispatch::DispatchTransport for LifetimeDeniedTransport {
    fn can_submit(&self, _: &webdock_mail_runtime::outbox::SendLease) -> bool {
        panic!("lifetime calibration must never prepare a provider send")
    }
    async fn submit(
        &self,
        _: &webdock_mail_runtime::outbox::SendLease,
        _: &serde_json::Value,
        _: &webdock_mail_runtime::message::DispatchMessage,
        _: &webdock_mail_runtime::message::PreparedMessage,
    ) -> webdock_mail_runtime::outbox::Receipt {
        panic!("lifetime calibration must never send to a provider")
    }
}
struct LifetimeDeniedActions;
#[tonic::async_trait]
impl webdock_plugin_runtime::ActionHandler for LifetimeDeniedActions {
    async fn invoke(
        &self,
        _: &webdock_plugin_protocol::InvocationClaims,
        _: &str,
        _: serde_json::Value,
        _: serde_json::Value,
    ) -> Result<serde_json::Value, webdock_plugin_protocol::PluginError> {
        panic!("lifetime calibration must never invoke a customer action")
    }
}
struct LifetimeServer {
    stop: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<Result<(), crate::Error>>,
}
impl Drop for LifetimeServer {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.task.abort();
    }
}
impl LifetimeServer {
    async fn finish(&mut self) -> bool {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        match tokio::time::timeout(Duration::from_secs(3), &mut self.task).await {
            Ok(result) => matches!(result, Ok(Ok(()))),
            Err(_) => {
                self.task.abort();
                let _ = (&mut self.task).await;
                false
            }
        }
    }
}
#[derive(Clone)]
struct LifetimeTls {
    origin: String,
    root: PathBuf,
    pin: String,
}
impl LifetimeTls {
    async fn channel(&self, role: &str) -> tonic::transport::Channel {
        let material = webdock_plugin_runtime::PinnedMutualTls {
            certificate: webdock_plugin_kit::read_file(
                &self.root.join(format!("{role}.crt")),
                false,
            )
            .unwrap(),
            private_key: webdock_plugin_kit::read_file(
                &self.root.join(format!("{role}.key")),
                true,
            )
            .unwrap(),
            authority: webdock_plugin_kit::read_file(&self.root.join("ca.crt"), false).unwrap(),
            server_certificate_sha256: self.pin.clone(),
        };
        let connected = tokio::time::timeout(
            Duration::from_secs(3),
            webdock_plugin_runtime::pinned_mtls_authenticated(
                self.origin.clone(),
                "localhost".into(),
                material,
            ),
        )
        .await;
        assert!(
            matches!(&connected, Ok(Ok(_))),
            "local pinned client connection failed"
        );
        connected.unwrap().unwrap().0
    }
    async fn rpc(&self) {
        let mut client =
            wire::plugin_runtime_client::PluginRuntimeClient::new(self.channel("core").await);
        let result = tokio::time::timeout(Duration::from_secs(3), async {
            let description = client.describe(wire::DescribeRequest {}).await;
            assert!(description.is_ok(), "local Core Describe denied");
            assert_eq!(
                description.unwrap().into_inner().manifest_json,
                webdock_plugin_protocol::Manifest::parse(include_bytes!("../manifest.json"))
                    .unwrap()
                    .as_bytes()
            );
            let health = client.health(wire::HealthRequest {}).await;
            assert!(health.is_ok(), "local Core Health denied");
            let health = health.unwrap().into_inner();
            assert_eq!(
                (
                    health.plugin_id.as_str(),
                    health.generation,
                    health.protocol_version
                ),
                ("mail", 2, 1)
            );
        })
        .await;
        assert!(result.is_ok(), "local fresh Core RPC deadline");
    }
}

#[tokio::test]
async fn actual_delivery_listener_survives_gateway_401_and_authorization_transition() {
    use std::os::unix::fs::PermissionsExt;
    use tower::ServiceExt;
    use webdock_plugin_runtime::{CorePeer, MutualTls, RemoteAuthority, Runtime};
    let mut f = Fixture::new().await;
    let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "Owned Mail fixture CA");
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::DigitalSignature,
    ];
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let ca = ca_params.self_signed(&ca_key).unwrap();
    let issuer = rcgen::Issuer::new(ca_params, ca_key);
    let mut pins = std::collections::BTreeMap::new();
    let mut runtime_identity = None;
    for role in ["runtime", "core", "gateway"] {
        let mut params = rcgen::CertificateParams::new(vec!["localhost".into()]).unwrap();
        params.distinguished_name.push(
            rcgen::DnType::CommonName,
            format!("Owned Mail fixture {role}"),
        );
        params.extended_key_usages = vec![
            rcgen::ExtendedKeyUsagePurpose::ServerAuth,
            rcgen::ExtendedKeyUsagePurpose::ClientAuth,
        ];
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = params.signed_by(&key, &issuer).unwrap();
        pins.insert(role, webdock_mail_protocol::sha256(cert.der()));
        for (suffix, bytes) in [
            ("crt", cert.pem().into_bytes()),
            ("key", key.serialize_pem().into_bytes()),
        ] {
            let path = f.directory.join(format!("{role}.{suffix}"));
            std::fs::write(&path, bytes).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        if role == "runtime" {
            runtime_identity = Some(tonic::transport::Identity::from_pem(
                cert.pem(),
                key.serialize_pem(),
            ));
        }
    }
    let ca_path = f.directory.join("ca.crt");
    std::fs::write(&ca_path, ca.pem()).unwrap();
    std::fs::set_permissions(ca_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_ne!(pins["core"], pins["gateway"]);
    let authority = RemoteAuthority::new(
        f.client.clone(),
        "mail".into(),
        2,
        vec![CorePeer::MutualTls {
            certificate_sha256: pins["core"].clone(),
        }],
    )
    .unwrap();
    let runtime = Runtime::new(
        webdock_plugin_protocol::Manifest::parse(include_bytes!("../manifest.json")).unwrap(),
        2,
        ed25519_dalek::SigningKey::from_bytes(&[71; 32]).verifying_key(),
        Arc::new(authority),
        Arc::new(LifetimeDeniedActions),
    )
    .unwrap();
    let control = Arc::new(LifetimeDeniedControl(AtomicUsize::new(0)));
    let payloads = Arc::new(LifetimeDeniedPayloads);
    let router = webdock_mail::listener::native_routes(
        webdock_mail_runtime::send_api::SendApi::new(control.clone(), payloads.clone()).router(),
        vec![CorePeer::MutualTls {
            certificate_sha256: pins["gateway"].clone(),
        }],
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tls = LifetimeTls {
        origin: format!("https://{}", listener.local_addr().unwrap()),
        root: f.directory.clone(),
        pin: pins["runtime"].clone(),
    };
    let material = MutualTls {
        identity: runtime_identity.unwrap(),
        peer_ca: tonic::transport::Certificate::from_pem(ca.pem()),
    };
    let (stop, stopped) = oneshot::channel();
    let (resumed, mut resumes) = mpsc::unbounded_channel();
    let client = f.client.clone();
    let worker_control = control.clone();
    let mut server =
        LifetimeServer {
            stop: Some(stop),
            task: tokio::spawn(super::serve_delivery(
                |drain| async move {
                    Ok(webdock_mail::listener::serve_mtls(
                        runtime,
                        router,
                        listener,
                        material,
                        async {
                            let _ = drain.await;
                        },
                    )
                    .await?)
                },
                async {
                    let _ = stopped.await;
                },
                client,
                "mail",
                2,
                move || {
                    let _ = resumed.send(());
                    std::future::ready(Ok::<(), ()>(()))
                },
                Some(super::Dispatcher {
                    control: worker_control,
                    payloads,
                    transport: Box::new(LifetimeDeniedTransport),
                }),
            )),
        };
    // Catch assertion unwinds in this task, then always signal and reap both owned local servers.
    let host_mode = f.host.mode.clone();
    let alive = server.task.abort_handle();
    let scenario = tokio::spawn(async move {
        let live = |stage: &'static str| {
            let server_live = !alive.is_finished();
            eprintln!("mail_listener_lifetime stage={stage} serverLive={server_live}");
            assert!(server_live, "local listener ended at {stage}");
        };
        live("started");
        let channel = tls.channel("gateway").await;
        let request = axum::http::Request::builder()
            .method("GET")
            .uri("/v1/emails/1")
            .body(tonic::body::Body::empty())
            .unwrap();
        let reply = tokio::time::timeout(Duration::from_secs(3), channel.oneshot(request)).await;
        assert!(matches!(&reply, Ok(Ok(_))), "local gateway request failed");
        let reply = reply.unwrap().unwrap();
        assert_eq!(reply.status(), axum::http::StatusCode::UNAUTHORIZED);
        live("gateway_401");
        assert_eq!(
            control.0.load(Ordering::SeqCst),
            0,
            "401 must precede control effects"
        );
        tls.rpc().await;
        live("inactive_fresh_rpc");
        assert!(
            resumes.try_recv().is_err(),
            "inactive authorization must not resume"
        );
        live("inactive_authorization");
        host_mode.store(1, Ordering::SeqCst);
        assert!(
            tokio::time::timeout(Duration::from_secs(3), resumes.recv())
                .await
                .is_ok(),
            "local active authorization did not resume"
        );
        live("active_resume");
        let polled = tokio::time::timeout(Duration::from_secs(1), async {
            while control.0.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(
            polled.is_ok(),
            "SomeDispatcher worker was not polled after resume"
        );
        tls.rpc().await;
        live("active_fresh_rpc");
    });
    let mut scenario = scenario;
    let result = tokio::time::timeout(Duration::from_secs(10), &mut scenario).await;
    let scenario_ok = match result {
        Ok(result) => result.is_ok(),
        Err(_) => {
            scenario.abort();
            let _ = scenario.await;
            false
        }
    };
    let clean = server.finish().await;
    f.task.abort();
    let _ = (&mut f.task).await;
    assert!(
        scenario_ok,
        "local prelude failed or exceeded its deadline; stage markers identify boundary"
    );
    assert!(clean, "local listener did not stop and reap cleanly");
    eprintln!("mail_listener_lifetime stage=explicit_shutdown_complete scope=unit_only");
}
