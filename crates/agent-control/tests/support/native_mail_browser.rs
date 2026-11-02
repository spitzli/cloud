use super::*;
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    time::Duration,
};
use webdock_control_plane::{
    http::{Services, build_app_with_domains},
    ui::UiAssets,
};
use webdock_identity::{RlsSigner, SecretKeys};

pub struct Fixture<'a> {
    pub incoming: Arc<NativeIncoming>,
    pub identity: Arc<IdentityService>,
    pub business: DatabaseConnection,
    pub auth: DatabaseConnection,
    pub config: AgentConfig,
    pub cookie: &'a str,
    pub directory: &'a Path,
    pub configure: bool,
}

pub async fn run(f: Fixture<'_>) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let dist = root.join("apps/console/dist");
    assert!(
        dist.join("index.html").is_file(),
        "Build the Console before acceptance"
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let ids = Arc::new(webdock_persistence::IdAllocator::new(
        f.business.clone(),
        webdock_ids::Identity::platform(0, 2).unwrap(),
    ));
    let access = webdock_access::AccessService::new(
        f.business,
        f.auth.clone(),
        f.identity.clone(),
        ids,
        RlsSigner::new(KEY, "webdock_auth_runtime").unwrap(),
        Arc::new(SecretKeys::new(BTreeMap::from([(1, KEY.into())]), 1, Some(KEY.into())).unwrap()),
        webdock_access::AccessConfig {
            auth_origin: origin.parse().unwrap(),
            console_origins: vec![origin.parse().unwrap()],
            allow_local_http: true,
        },
    )
    .unwrap();
    let config = webdock_core::config::PlatformConfig::from_map(&HashMap::from([
        ("WEBDOCK_STUDIO_ORIGIN".into(), origin.clone()),
        ("WEBDOCK_FEATURES".into(), r#"{"mail":"enabled"}"#.into()),
    ]))
    .unwrap();
    let router = build_app_with_domains(
        config,
        f.auth,
        Services {
            identity: Some(f.identity),
            mail: Some(Arc::new(
                webdock_control_plane::mail::MailEndpoints::incoming_only(f.incoming),
            )),
            access: Some(Arc::new(access)),
            ui: Some(Arc::new(UiAssets::new(&dist, &[], &[], false).unwrap())),
            ..Default::default()
        },
    );
    let (name, value) = f.cookie.split_once('=').unwrap();
    let descriptor = f.directory.join("browser.json");
    private(
        &descriptor,
        &json!({"origin":origin,"cookie":{"name":name,"value":value},"customerID":"123","customerName":"Native Mail fixture","configure":f.configure}),
    );
    let (stop_http, shutdown_http) = tokio::sync::oneshot::channel();
    let http = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = shutdown_http.await;
            })
            .await
            .unwrap();
    });
    let (stop_agent, shutdown_agent) = tokio::sync::oneshot::channel();
    let daemon = Daemon::open(f.config).await.unwrap();
    let agent = tokio::spawn(async move {
        daemon
            .run(async {
                let _ = shutdown_agent.await;
            })
            .await
            .unwrap();
    });
    let result = tokio::time::timeout(
        Duration::from_secs(if f.configure { 300 } else { 180 }),
        tokio::process::Command::new("node")
            .arg(root.join("apps/console/tests/mail-native-browser.mjs"))
            .env("WEBDOCK_MAIL_BROWSER_FIXTURE", descriptor)
            .current_dir(&root)
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let _ = stop_agent.send(());
    let _ = stop_http.send(());
    agent.await.unwrap();
    http.await.unwrap();
    let output = result
        .expect("Native Mail browser deadline")
        .expect("Native Mail browser process");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{}", String::from_utf8_lossy(&output.stdout));
}
