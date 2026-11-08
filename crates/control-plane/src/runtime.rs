//! Native HTTP lifecycle; no framework queue, migration or account bootstrap hooks.
use axum::Router;
use std::{
    future::{Future, IntoFuture},
    io,
    time::Duration,
};
use tokio::{net::TcpListener, sync::oneshot};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Message(String),
    #[error("Native HTTP runtime I/O failed")]
    Io(#[from] io::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

/// Existing requests drain, but an open SSE client cannot prevent process shutdown forever.
pub async fn serve_with_shutdown(
    listener: TcpListener,
    app: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
    grace: Duration,
) -> io::Result<()> {
    if grace.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Shutdown grace must be bounded and positive",
        ));
    }
    let (begin, begun) = oneshot::channel();
    let server = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown.await;
            let _ = begin.send(());
        })
        .into_future();
    tokio::pin!(server);
    tokio::select! {
        result = &mut server => result,
        started = begun => {
            started.map_err(|_| io::Error::other("Shutdown listener ended unexpectedly"))?;
            tokio::time::timeout(grace, &mut server).await
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "HTTP shutdown grace expired"))?
        }
    }
}

#[cfg(unix)]
pub fn shutdown_signal() -> io::Result<impl Future<Output = ()> + Send + 'static> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    Ok(async move {
        tokio::select! { _ = terminate.recv() => {}, _ = interrupt.recv() => {} }
    })
}

#[cfg(not(unix))]
pub fn shutdown_signal() -> io::Result<impl Future<Output = ()> + Send + 'static> {
    Ok(async {
        if tokio::signal::ctrl_c().await.is_err() {
            tracing::error!("Shutdown signal listener failed");
        }
    })
}
