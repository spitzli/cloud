//! Dispatch both durable account outboxes through the explicitly configured native Mail API.
use std::{path::Path, time::Duration};
use webdock_mail::account_delivery::{
    AccountDispatcher, DispatcherConfiguration, read_configuration,
};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if !(args.len() == 1 || (args.len() == 2 && args[1] == "--once")) {
        return Err("Usage: webdock-account-delivery /absolute/private.json [--once]".into());
    }
    let config: DispatcherConfiguration =
        serde_json::from_slice(&read_configuration(Path::new(&args[0]))?)
            .map_err(|_| "Invalid private account delivery configuration")?;
    let dispatcher = AccountDispatcher::connect(config).await?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    loop {
        let dispatch = dispatcher.dispatch_once();
        tokio::pin!(dispatch);
        let (result, stopping) = tokio::select! {result=&mut dispatch=>(result,false),_=terminate.recv()=>(dispatch.await,true),result=tokio::signal::ctrl_c()=>{result?;(dispatch.await,true)}};
        if result.is_err() {
            eprintln!("Account delivery unavailable; inspect durable outbox");
        }
        if stopping || args.len() == 2 {
            return result.map_err(Into::into);
        }
        tokio::select! {_=tokio::time::sleep(Duration::from_secs(1))=>{},_=terminate.recv()=>return Ok(()),result=tokio::signal::ctrl_c()=>{result?;return Ok(());}}
    }
}
