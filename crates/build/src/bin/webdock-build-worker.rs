use webdock_build::worker::WorkerConfig;
#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("--config") {
        eprintln!("Usage: webdock-build-worker --config /private/config.json [--once]");
        std::process::exit(2);
    }
    let Some(path) = args.next() else {
        std::process::exit(2)
    };
    let once = match args.next().as_deref() {
        Some("--once") => true,
        None => false,
        _ => std::process::exit(2),
    };
    if args.next().is_some() {
        std::process::exit(2);
    }
    let worker = match WorkerConfig::load(std::path::Path::new(&path)).and_then(WorkerConfig::build)
    {
        Ok(worker) => worker,
        Err(error) => {
            eprintln!("{}", error.code);
            std::process::exit(1);
        }
    };
    let mut prefer_release = false;
    loop {
        let outcome =
            tokio::select! {r=worker.turn(prefer_release)=>r,_=tokio::signal::ctrl_c()=>return};
        let worked = match outcome {
            Ok(worked) => worked,
            Err(error) => {
                eprintln!("{}", error.code);
                if error.code == "worker_revoked" {
                    std::process::exit(1);
                }
                false
            }
        };
        if once {
            return;
        }
        prefer_release = !prefer_release;
        if !worked {
            tokio::select! {_=tokio::time::sleep(std::time::Duration::from_secs(5))=>{},_=tokio::signal::ctrl_c()=>return}
        }
    }
}
