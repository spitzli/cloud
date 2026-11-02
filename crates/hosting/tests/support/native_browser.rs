//! Actual built Console → native HTTP/RLS → persisted agent RPC → isolated Docker.
use super::*;
use std::{
    collections::{BTreeMap, HashMap},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
    time::Duration,
};
use webdock_agent::config::{AgentConfig, ControlEndpoint, RuntimeConfiguration};
use webdock_agent_control::{ControlPlane, LocalBinding};
use webdock_agent_protocol::agent_control_server::AgentControlServer;
use webdock_control_plane::{
    http::{Services, build_app_with_domains},
    ui::UiAssets,
};
use webdock_identity::{RlsSigner, SecretKeys};

const AGENT: &str = "browser-fixture";

async fn http(f: &Fixture, origin: &str, dist: &Path) -> (axum::Router, Value) {
    let cookies = CookiePolicy::new(false, "fixture-hosting-cookie-key-at-least-32-bytes").unwrap();
    let cookie = json!({
        "name":cookies.name(),
        "value":cookies.sign(&format!("{}-token", f.nonce)).unwrap()
    });
    let auth = db("AUTH_DATABASE_URL").await;
    let business = db("DATABASE_URL").await;
    let identity = Arc::new(IdentityService::new(auth.clone(), cookies));
    let secret = std::env::var("BETTER_AUTH_SECRET").unwrap();
    let rls = std::env::var("DATABASE_RLS_SECRET").unwrap();
    let ids = Arc::new(webdock_persistence::IdAllocator::new(
        business.clone(),
        webdock_ids::Identity::platform(0, 2).unwrap(),
    ));
    let access = webdock_access::AccessService::new(
        business,
        auth.clone(),
        identity.clone(),
        ids.clone(),
        RlsSigner::new(&rls, "webdock_auth_runtime").unwrap(),
        Arc::new(SecretKeys::new(BTreeMap::from([(1, secret.clone())]), 1, Some(secret)).unwrap()),
        webdock_access::AccessConfig {
            auth_origin: "http://localhost:3125".parse().unwrap(),
            console_origins: vec![origin.parse().unwrap()],
            allow_local_http: true,
        },
    )
    .unwrap();
    let registry = webdock_registry::RegistryService::new(
        db("WEBDOCK_REGISTRY_DATABASE_URL").await,
        RlsSigner::new(&rls, "webdock_registry_runtime").unwrap(),
    )
    .with_allocator(ids);
    let config = webdock_core::config::PlatformConfig::from_map(&HashMap::from([
        ("WEBDOCK_STUDIO_ORIGIN".into(), origin.into()),
        ("WEBDOCK_FEATURES".into(), r#"{"hosting":"enabled"}"#.into()),
    ]))
    .unwrap();
    let router = build_app_with_domains(
        config,
        auth,
        Services {
            identity: Some(identity),
            registry: Some(Arc::new(registry)),
            hosting: Some(f.service.clone()),
            access: Some(Arc::new(access)),
            ui: Some(Arc::new(UiAssets::new(dist, &[], &[], false).unwrap())),
            ..Default::default()
        },
    );
    (router, cookie)
}

async fn enroll(f: &Fixture, socket: &Path) -> (String, ControlPlane) {
    let installation: String = f
        .owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT runtime_installation FROM webdock_auth.hosting_cluster WHERE id=$1",
            [f.cluster.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "runtime_installation")
        .unwrap();
    let uid = rustix::process::geteuid().as_raw();
    sql(
        &f.owner,
        "UPDATE webdock_auth.hosting_agent SET last_sequence=0 WHERE cluster_id=$1",
        vec![f.cluster.clone().into()],
    )
    .await;
    sql(&f.owner, "INSERT INTO webdock_auth.agent_transport_enrollment(installation,agent_id,generation,cluster_id,credential_hash,transport,unix_uid) VALUES($1,$2,1,$3,$4,'unix',$5)",
        vec![installation.clone().into(),AGENT.into(),f.cluster.clone().into(),
            hex::encode(Sha256::digest("a".repeat(43))).into(),i64::from(uid).into()]).await;
    assert!(!socket.exists());
    let control = ControlPlane::new(
        db("AUTH_DATABASE_URL").await,
        f.service.clone(),
        Some(LocalBinding {
            installation: installation.clone(),
            agent_id: AGENT.into(),
            uid,
        }),
    )
    .await
    .unwrap();
    (installation, control)
}

const CREATED: &str = "browser-created";

async fn remove_runtime(engine: &bollard::Docker, installation: &str, app: &str) {
    let name = format!("wd-{installation}-{app}");
    match engine.inspect_container(&name, None).await {
        Ok(container) => {
            let labels = container.config.unwrap().labels.unwrap();
            assert_eq!(
                labels.get("webdock.dev/installation").map(String::as_str),
                Some(installation)
            );
            assert_eq!(
                labels.get("webdock.dev/resource-id").map(String::as_str),
                Some(app)
            );
            engine
                .remove_container(
                    &name,
                    Some(bollard::query_parameters::RemoveContainerOptions {
                        force: true,
                        ..Default::default()
                    }),
                )
                .await
                .unwrap();
        }
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => {}
        Err(error) => panic!("Cannot verify owned browser runtime: {error}"),
    }
    let network = format!("{name}-net");
    match engine.inspect_network(&network, None).await {
        Ok(network_state) => {
            let labels = network_state.labels.unwrap();
            assert_eq!(
                labels.get("webdock.dev/installation").map(String::as_str),
                Some(installation)
            );
            assert_eq!(
                labels.get("webdock.dev/resource-id").map(String::as_str),
                Some(app)
            );
            engine.remove_network(&network).await.unwrap();
        }
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => {}
        Err(error) => panic!("Cannot verify owned browser network: {error}"),
    }
}

#[tokio::test]
#[ignore = "requires owned PostgreSQL, local Docker, built Console and explicit browser opt-in"]
async fn console_drives_real_native_hosting_and_docker() {
    assert_eq!(
        std::env::var("WEBDOCK_HOSTING_BROWSER_TEST").as_deref(),
        Ok("1")
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let dist = root.join("apps/console/dist");
    assert!(dist.join("index.html").is_file(), "Build the Console first");
    let engine = bollard::Docker::connect_with_unix(
        "/var/run/docker.sock",
        10,
        bollard::API_DEFAULT_VERSION,
    )
    .unwrap();
    engine
        .inspect_image(spec()["image"].as_str().unwrap())
        .await
        .expect("The explicit digest-pinned fixture image must already exist locally");
    let fixture = Arc::new(Fixture::new("docker").await);
    fixture.configure("docker").await;
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let state = directory.path().join("state");
    std::fs::create_dir(&state).unwrap();
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = directory.path().join("agent.sock");
    let (installation, control) = enroll(&fixture, &socket).await;
    let rpc = tokio::net::UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let (stop_rpc, rpc_shutdown) = tokio::sync::oneshot::channel();
    let rpc_task = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(AgentControlServer::new(control))
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::UnixListenerStream::new(rpc),
                async {
                    let _ = rpc_shutdown.await;
                },
            )
            .await
            .unwrap();
    });
    let config = AgentConfig {
        version: 1,
        installation: installation.clone(),
        agent_id: AGENT.into(),
        generation: 1,
        state_directory: state,
        control: ControlEndpoint::Unix { path: socket },
        runtime: RuntimeConfiguration::Docker {
            socket: "/var/run/docker.sock".into(),
            initializer_image: None,
            storage_socket: None,
        },
        database_runtime: None,
        mail_configuration_enabled: false,
        mail_runtime: None,
        recovery_runtime: None,
        policy: Default::default(),
        workloads: false,
        plugin_runtime_enabled: false,
    };
    let captured_app = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let captured = captured_app.clone();
    let f = fixture.clone();
    let probe = (engine.clone(), installation.clone());
    let outcome = tokio::spawn(async move {
        let mut daemon = webdock_agent::management::Daemon::open(config)
            .await
            .unwrap();
        daemon.connect().await.unwrap();
        let mut runtime_spec = spec();
        runtime_spec["args"] = json!([
            "sh",
            "-c",
            "echo browser-runtime-log; printf ok >/tmp/index.html; exec httpd -f -p 8080 -h /tmp"
        ]);
        runtime_spec["ephemeralBytes"] = json!(16777216);
        let created = f.service.execute(&f.principal, json!({
            "action":"apps.create","projectID":f.pid,"name":"browser-runtime",
            "spec":runtime_spec,"subscriptionRevision":1,"idempotencyKey":"browser-create-runtime"
        })).await.unwrap();
        let app = created["appID"].as_str().unwrap().to_owned();
        captured.lock().unwrap().push(app.clone());
        daemon.cycle().await.unwrap();
        let view = f
            .service
            .execute(&f.principal, json!({"action":"apps.get","appID":app}))
            .await
            .unwrap();
        assert_eq!(view["status"], "ready");
        // Room for exactly one Console-created application beside the fixture runtime.
        sql(&f.owner,"UPDATE webdock_auth.tenant_subscription SET base=jsonb_set(base,'{hosting,apps}','2'),revision=revision+1 WHERE customer_id=$1",vec![f.cid.clone().into()]).await;
        sql(&f.owner,"UPDATE webdock_auth.hosting_cluster SET capacity=jsonb_set(capacity,'{apps}','2') WHERE id=$1",vec![f.cluster.clone().into()]).await;
        run_browser(&f, daemon, &app, root, &dist, directory.path()).await;
        // The Console-created application ran and was then deleted through the reviewed plan.
        let created = f
            .owner
            .query_one_raw(sea_orm::Statement::from_sql_and_values(
                sea_orm::DbBackend::Postgres,
                "SELECT id,status FROM webdock_auth.hosting_app WHERE project_id=$1 AND name=$2",
                [f.pid.clone().into(), CREATED.into()],
            ))
            .await
            .unwrap()
            .expect("Console must have created the second application");
        let created_id: String = created.try_get("", "id").unwrap();
        captured.lock().unwrap().push(created_id.clone());
        assert_eq!(created.try_get::<String>("", "status").unwrap(), "deleted");
        assert!(
            matches!(
                probe
                    .0
                    .inspect_container(&format!("wd-{}-{created_id}", probe.1), None)
                    .await,
                Err(bollard::errors::Error::DockerResponseServerError {
                    status_code: 404,
                    ..
                })
            ),
            "Deleted application container must be removed"
        );
        // The write-only browser patch must reach the redeployed container, not just the database.
        let container = probe
            .0
            .inspect_container(&format!("wd-{}-{app}", probe.1), None)
            .await
            .unwrap();
        let env = container.config.unwrap().env.unwrap_or_default();
        assert!(
            env.iter()
                .any(|value| value == "WEBDOCK_PROOF_TOKEN=browser-proof-value"),
            "Console environment change did not reach the running container"
        );
    })
    .await;
    let _ = stop_rpc.send(());
    rpc_task.await.unwrap();
    let apps = captured_app.lock().unwrap().clone();
    for app in apps {
        remove_runtime(&engine, &installation, &app).await;
    }
    for table in ["agent_runtime_dispatch", "agent_transport_enrollment"] {
        sql(
            &fixture.owner,
            &format!("DELETE FROM webdock_auth.{table} WHERE installation=$1 AND agent_id=$2"),
            vec![installation.clone().into(), AGENT.into()],
        )
        .await;
    }
    fixture.cleanup().await;
    outcome.unwrap();
}

async fn run_browser(
    f: &Fixture,
    daemon: webdock_agent::management::Daemon,
    app: &str,
    root: &Path,
    dist: &Path,
    directory: &Path,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (router, cookie) = http(f, &origin, dist).await;
    let (stop_http, http_shutdown) = tokio::sync::oneshot::channel();
    let http_task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = http_shutdown.await;
            })
            .await
            .unwrap();
    });
    let descriptor = directory.join("browser.json");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&descriptor)
        .unwrap();
    file.write_all(
        serde_json::to_string(&json!({
            "origin":origin,"cookie":cookie,"customerID":f.cid,"customerName":f.nonce,
            "projectID":f.pid,"projectName":f.nonce,"appID":app,"appName":"browser-runtime",
            "updateCPU":"0.2 cores","expectedLog":"browser-runtime-log",
            "createName":CREATED,"createImage":spec()["image"],
            "createArgs":["sh","-c","printf ok >/tmp/index.html; exec httpd -f -p 8080 -h /tmp"]
        }))
        .unwrap()
        .as_bytes(),
    )
    .unwrap();
    file.sync_all().unwrap();
    let (stop_daemon, daemon_shutdown) = tokio::sync::oneshot::channel();
    let daemon_task = tokio::spawn(async move {
        daemon
            .run(async {
                let _ = daemon_shutdown.await;
            })
            .await
            .unwrap();
    });
    let output = tokio::time::timeout(
        Duration::from_secs(300),
        tokio::process::Command::new("node")
            .arg(root.join("apps/console/tests/hosting-native-browser.mjs"))
            .env("WEBDOCK_HOSTING_BROWSER_FIXTURE", &descriptor)
            .current_dir(root)
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let _ = stop_daemon.send(());
    let _ = stop_http.send(());
    daemon_task.await.unwrap();
    http_task.await.unwrap();
    let output = output
        .expect("Native browser timeout")
        .expect("Native browser process");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{}", String::from_utf8_lossy(&output.stdout));
}
