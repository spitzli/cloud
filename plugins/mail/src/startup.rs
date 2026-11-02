//! Listener-scoped retained reconciliation: candidates can be probed without customer effects.
use std::{future::Future, time::Duration};
use webdock_plugin_runtime::HostClient;

#[derive(Clone, Copy)]
pub(super) enum LifetimeEvent {
    Sigterm,
    Sigint,
    SignalApiError,
    SignalStreamClosed,
    ServerOk,
    ServerErr,
    RequestedShutdown,
    DispatcherStopped,
}
impl LifetimeEvent {
    fn token(self) -> &'static str {
        match self {
            Self::Sigterm => "SIGTERM",
            Self::Sigint => "SIGINT",
            Self::SignalApiError => "signal_api_error",
            Self::SignalStreamClosed => "signal_stream_closed",
            Self::ServerOk => "server_ok",
            Self::ServerErr => "server_err",
            Self::RequestedShutdown => "requested_shutdown",
            Self::DispatcherStopped => "dispatcher_stopped",
        }
    }
    fn line(self, flag: Option<&std::ffi::OsStr>) -> Option<String> {
        (flag == Some(std::ffi::OsStr::new("1")))
            .then(|| format!("mail_lifetime event={}\n", self.token()))
    }
    pub(super) fn emit(self) {
        use std::io::Write;
        if let Some(line) =
            self.line(std::env::var_os("WEBDOCK_RECOVERY_COMPONENT_TEST").as_deref())
        {
            let _ = std::io::stderr().write_all(line.as_bytes());
        }
    }
    fn server(result: &Result<(), crate::Error>) {
        if result.is_ok() {
            Self::ServerOk
        } else {
            Self::ServerErr
        }
        .emit();
    }
}

pub(super) async fn serve<S, H, F, R, E>(
    server: impl FnOnce(tokio::sync::oneshot::Receiver<()>) -> S,
    shutdown: H,
    client: HostClient,
    plugin: &str,
    generation: u64,
    resume: F,
) -> Result<(), crate::Error>
where
    S: Future<Output = Result<(), crate::Error>>,
    H: Future<Output = ()>,
    F: FnMut() -> R,
    R: Future<Output = Result<(), E>>,
{
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let mut server = std::pin::pin!(server(stopped));
    let mut shutdown = std::pin::pin!(shutdown);
    {
        let mut startup =
            std::pin::pin!(resume_when_authorized(client, plugin, generation, resume));
        tokio::select! {
            biased;
            () = &mut shutdown => { LifetimeEvent::RequestedShutdown.emit(); }
            result = &mut server => { LifetimeEvent::server(&result); return result; },
            () = &mut startup => {
                tokio::select! {
                    biased;
                    () = &mut shutdown => { LifetimeEvent::RequestedShutdown.emit(); }
                    result = &mut server => { LifetimeEvent::server(&result); return result; },
                }
            }
        }
    }
    // Drop startup before asking tonic to drain: active requests must not prolong its authority.
    let _ = stop.send(());
    let result = server.await;
    LifetimeEvent::server(&result);
    result
}

async fn resume_when_authorized<F, R, E>(
    client: HostClient,
    plugin: &str,
    generation: u64,
    mut resume: F,
) where
    F: FnMut() -> R,
    R: Future<Output = Result<(), E>>,
{
    let mut backoff = Duration::from_millis(100);
    loop {
        if client.authorize_runtime(plugin, generation).await.is_ok() {
            if resume().await.is_ok() {
                return;
            }
            eprintln!("mail_plugin_resume_pending");
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(5));
    }
}

#[cfg(test)]
#[path = "startup_tests.rs"]
mod tests;

pub(super) struct Dispatcher {
    pub control: std::sync::Arc<dyn webdock_mail_runtime::SendControl>,
    pub payloads: std::sync::Arc<dyn webdock_mail_runtime::PrivatePayloadStore>,
    #[cfg(not(test))]
    pub transport: webdock_mail::executor::ManagedTransport,
    #[cfg(test)]
    pub transport: Box<dyn webdock_mail_runtime::dispatch::DispatchTransport>,
}
/// One listener-owned sender future. Shutdown prevents a new claim and bounds the in-flight
/// attempt/receipt drain; dropping it after handoff leaves Core's conservative unknown state.
pub(super) async fn serve_delivery<S, H, F, R, E>(
    server: impl FnOnce(tokio::sync::oneshot::Receiver<()>) -> S,
    shutdown: H,
    client: HostClient,
    plugin: &str,
    generation: u64,
    resume: F,
    delivery: Option<Dispatcher>,
) -> Result<(), crate::Error>
where
    S: Future<Output = Result<(), crate::Error>>,
    H: Future<Output = ()>,
    F: FnMut() -> R,
    R: Future<Output = Result<(), E>>,
{
    let Some(Dispatcher {
        control,
        payloads,
        transport,
    }) = delivery
    else {
        return serve(server, shutdown, client, plugin, generation, resume).await;
    };
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let (worker_stop, worker_stopped) = tokio::sync::watch::channel(false);
    let mut server = std::pin::pin!(server(stopped));
    let mut shutdown = std::pin::pin!(shutdown);
    let mut worker = std::pin::pin!(async {
        let mut startup = std::pin::pin!(resume_when_authorized(
            client.clone(),
            plugin,
            generation,
            resume
        ));
        let mut stopped = worker_stopped.clone();
        tokio::select! {biased;_=stopped.changed()=>return,()=&mut startup=>{}}
        loop {
            if *stopped.borrow() {
                return;
            }
            let authorized = tokio::select! {biased;_=stopped.changed()=>return,result=client.authorize_runtime(plugin,generation)=>result.is_ok()};
            if authorized && !*stopped.borrow() {
                // This future is deliberately retained through shutdown for bounded finalization.
                let _ =
                    webdock_mail_runtime::dispatch::dispatch_one(&*control, payloads.clone(), {
                        #[cfg(not(test))]
                        {
                            &transport
                        }
                        #[cfg(test)]
                        {
                            transport.as_ref()
                        }
                    })
                    .await;
                if *stopped.borrow() {
                    return;
                }
                let _ =
                    webdock_mail_runtime::dispatch::cleanup_payloads(&*control, payloads.clone())
                        .await;
            }
            tokio::select! {biased;_=stopped.changed()=>return,()=tokio::time::sleep(Duration::from_millis(250))=>{}}
        }
    });
    tokio::select! {biased;()=&mut shutdown=>{LifetimeEvent::RequestedShutdown.emit();},result=&mut server=>{LifetimeEvent::server(&result);return result;},()=&mut worker=>{LifetimeEvent::DispatcherStopped.emit();return Err("mail dispatcher stopped".into());}}
    let _ = worker_stop.send(true);
    let _ = stop.send(());
    // 15s provider deadline + up to two15s Core calls, with a small drain allowance.
    let drain = async { tokio::join!(&mut server, &mut worker).0 };
    let result = tokio::time::timeout(Duration::from_secs(50), drain)
        .await
        .map_err(|_| "mail drain deadline")?;
    LifetimeEvent::server(&result);
    result
}
