//! Genuine Core admission component: no SQL actors, sessions, module, quota or Ready seeds.
use super::*;
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    os::unix::{
        fs::{DirBuilderExt, OpenOptionsExt},
        process::ExitStatusExt,
    },
    process::Stdio,
};
#[path = "recovery_app_component.rs"]
mod app_component;
#[path = "recovery_byte_component.rs"]
mod byte_component;
#[path = "recovery_mail_component.rs"]
pub(super) mod mail_component;

pub(super) async fn fixture() -> Fixture {
    assert_eq!(
        std::env::var("WEBDOCK_RECOVERY_COMPONENT_TEST").as_deref(),
        Ok("1")
    );
    assert_eq!(rustix::process::geteuid().as_raw(), 0);
    assert!(Path::new("/.dockerenv").is_file());
    if mail_component::enabled() {
        return mail_component::core_fixture().await;
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let password = format!("{nonce}{}", uuid::Uuid::new_v4().simple());
    let postgres_image = std::env::var("WEBDOCK_RECOVERY_POSTGRES_IMAGE_ID").unwrap();
    assert!(
        postgres_image
            .strip_prefix("sha256:")
            .is_some_and(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()))
    );
    let created = docker()
        .args([
            "run",
            "--detach",
            "--name",
            &format!("owned-recovery-core-{}", &nonce[..12]),
            "--label",
            &format!("webdock.rust-operator={nonce}"),
            "--network",
            "host",
            "--memory",
            "512m",
            "--memory-swap",
            "512m",
            "--cpus",
            "1",
            "--pids-limit",
            "128",
            "--env",
            "POSTGRES_PASSWORD",
            "--env",
            "POSTGRES_DB=webdock_operator_test",
            "--pull=never",
            &postgres_image,
            "postgres",
            "-h",
            "127.0.0.1",
            "-p",
            &port.to_string(),
        ])
        .env("POSTGRES_PASSWORD", &password)
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "owned namespace-local PostgreSQL startup"
    );
    let container = String::from_utf8(created.stdout).unwrap().trim().to_owned();
    assert_eq!(container.len(), 64);
    let mut url = url::Url::parse(&format!(
        "postgresql://postgres@127.0.0.1:{port}/webdock_operator_test"
    ))
    .unwrap();
    url.set_password(Some(&password)).unwrap();
    let passwords = webdock_persistence::roles::LOGIN_ROLES
        .iter()
        .copied()
        .chain(["webdock_offline_migrate"])
        .map(|role| {
            (
                role.into(),
                format!(
                    "{}{}",
                    uuid::Uuid::new_v4().simple(),
                    uuid::Uuid::new_v4().simple()
                ),
            )
        })
        .collect();
    let fixture = Fixture {
        container,
        url,
        passwords,
    };
    let owner = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(db) = connect(fixture.url.as_str()).await {
                break db;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    webdock_persistence::roles::provision_roles(
        &owner,
        "webdock_operator_test",
        &fixture.passwords,
    )
    .await
    .unwrap();
    webdock_persistence::migrations::bootstrap(&owner, "webdock_operator_test", SECRET)
        .await
        .unwrap();
    owner.close().await.unwrap();
    fixture
}
pub(super) fn enabled() -> bool {
    std::env::var("WEBDOCK_RECOVERY_COMPONENT_TEST").as_deref() == Ok("1")
}
pub(super) enum RecoveryPhase {
    OfflineBootstrap,
    PasswordSignIn,
    PasswordChanged,
    TotpEnrolled,
    TotpVerified,
    PkceConsoleOperator,
    HostingActivated,
    RegistryCustomerCreated,
    RegistryProjectCreated,
    AllowancesGranted,
    AgentEnrolled,
    ControlSocketBound,
    StorageInitialized,
    AgentSpawned,
    AgentHeartbeatObserved,
    CapacityActivated,
    HostingProjectConfigured,
    SourcePrepared,
    DraftPolicyDenied,
    SourceDeployQueued,
}
pub(super) fn phase(stage: RecoveryPhase) {
    if enabled() {
        let label = match stage {
            RecoveryPhase::OfflineBootstrap => "offline_bootstrap_succeeded",
            RecoveryPhase::PasswordSignIn => "password_signin_succeeded",
            RecoveryPhase::PasswordChanged => "password_change_succeeded",
            RecoveryPhase::TotpEnrolled => "totp_enrollment_succeeded",
            RecoveryPhase::TotpVerified => "totp_verification_succeeded",
            RecoveryPhase::PkceConsoleOperator => "pkce_s256_console_operator_succeeded",
            RecoveryPhase::HostingActivated => "builtin_hosting_activation_succeeded",
            RecoveryPhase::RegistryCustomerCreated => "registry_customer_creation_succeeded",
            RecoveryPhase::RegistryProjectCreated => "registry_project_creation_succeeded",
            RecoveryPhase::AllowancesGranted => "finite_allowance_succeeded",
            RecoveryPhase::AgentEnrolled => "offline_agent_enrollment_succeeded",
            RecoveryPhase::ControlSocketBound => "control_socket_bound",
            RecoveryPhase::StorageInitialized => "storage_initialize_succeeded",
            RecoveryPhase::AgentSpawned => "agent_process_spawned",
            RecoveryPhase::AgentHeartbeatObserved => "actual_agent_heartbeat_observed",
            RecoveryPhase::CapacityActivated => "cluster_capacity_activation_succeeded",
            RecoveryPhase::HostingProjectConfigured => "hosting_project_creation_succeeded",
            RecoveryPhase::SourcePrepared => "source_draft_prepared",
            RecoveryPhase::DraftPolicyDenied => "unconfigured_draft_policy_denial_observed",
            RecoveryPhase::SourceDeployQueued => "source_deploy_queued",
        };
        eprintln!("recovery_component phase={label}");
    }
}
fn private(path: &Path, value: &impl serde::Serialize) {
    std::fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
fn directory(path: &Path) {
    std::fs::create_dir(path).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}
pub(super) fn with_signer(
    hosting: Arc<webdock_hosting::HostingService>,
    path: &Path,
    identity: &Arc<webdock_identity::IdentityService>,
    browser: &Arc<webdock_browser_session::BrowserSessions>,
) -> Arc<webdock_hosting::HostingService> {
    if !enabled() {
        return hosting;
    }
    let key = path.join("recovery-signing");
    assert!(
        Command::new("/usr/bin/ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&key)
            .status()
            .unwrap()
            .success()
    );
    let hosting = Arc::try_unwrap(hosting).ok().unwrap();
    Arc::new(
        hosting
            .with_session_authority(identity.clone(), Some(browser.clone()))
            .with_recovery_signer(
                webdock_hosting::RecoverySigner::new(key, path.to_path_buf()).unwrap(),
            ),
    )
}
async fn post(app: &Router, console: &str, body: Value) -> Reply {
    request(app, CONSOLE, "POST", "/api/console/hosting", console, body).await
}
fn okay(reply: Reply, stage: &str) -> Value {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "recovery component stage={stage} body={}",
        reply.body
    );
    reply.body
}
fn spawn(
    binary: &str,
    args: &[&std::ffi::OsStr],
    directory: &Path,
    name: &str,
) -> tokio::process::Child {
    let result = (|| -> io::Result<tokio::process::Child> {
        let out = File::create(directory.join(format!("{name}.stdout")))?;
        let err = File::create(directory.join(format!("{name}.stderr")))?;
        tokio::process::Command::new(binary)
            .args(args)
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err))
            .kill_on_drop(true)
            .spawn()
    })();
    if let Err(error) = &result
        && enabled()
    {
        let participant = match name {
            "agent-first" => "agent",
            "storage" => "storage_helper",
            _ => "unknown",
        };
        let output = Path::new("/diagnostics")
            .join(format!("native-startup-{}", uuid::Uuid::new_v4().simple()));
        let retained = retain_heartbeat_diagnostics(
            directory,
            &output,
            json!({"startup":{"participant":participant,"errorKind":format!("{:?}",error.kind())},"controlResult":"not_sampled"}),
        );
        eprintln!(
            "recovery_component startup_diagnostics={}",
            if retained.is_ok() {
                "retained_categories_only"
            } else {
                "unavailable"
            }
        );
    }
    result.unwrap()
}
async fn stop(child: &mut tokio::process::Child) {
    if child.try_wait().unwrap().is_none() {
        child.start_kill().unwrap();
    }
    child.wait().await.unwrap();
}
async fn row(owner: &impl ConnectionTrait, query: &str, values: Vec<sea_orm::Value>) -> Value {
    owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            query,
            values,
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "value")
        .unwrap()
}
async fn wait_agent(owner: &DatabaseConnection, cluster: &str, recovery: bool) -> Value {
    tokio::time::timeout(Duration::from_secs(40), async {
        loop {
            let value = row(
                owner,
                "SELECT to_jsonb(a) AS value FROM webdock_auth.hosting_agent a WHERE cluster_id=$1",
                vec![cluster.into()],
            )
            .await;
            if !value["last_seen"].is_null()
                && (!recovery
                    || value["observation"]["capabilities"]["nativeRecovery"]["version"] == 2)
            {
                break value;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("actual enrolled native agent heartbeat")
}
pub(super) async fn check(
    app: &Router,
    console: &str,
    fixture: &Fixture,
    hosting: &Arc<webdock_hosting::HostingService>,
    (auth, owner): (&DatabaseConnection, &DatabaseConnection),
    path: &Path,
    mail_dependencies: Option<mail_component::FoundationDependencies>,
) {
    assert_eq!(
        rustix::process::geteuid().as_raw(),
        0,
        "only the explicitly owned isolated fixture"
    );
    assert!(Path::new("/.dockerenv").is_file());
    okay(
        activate(app, console, HOSTING, "recovery-hosting-foundation").await,
        "activate-hosting",
    );
    phase(RecoveryPhase::HostingActivated);
    let customer = okay(
        request(
            app,
            CONSOLE,
            "POST",
            "/api/console/registry/customers",
            console,
            json!({"name":"Owned native recovery component","country":"de"}),
        )
        .await,
        "customer",
    );
    let customer = customer["id"].as_str().unwrap();
    phase(RecoveryPhase::RegistryCustomerCreated);
    let project = okay(
        request(
            app,
            CONSOLE,
            "POST",
            "/api/console/registry/projects",
            console,
            json!({"name":"Owned ntfy recovery source","customer":customer}),
        )
        .await,
        "project",
    );
    let project = project["id"].as_str().unwrap();
    phase(RecoveryPhase::RegistryProjectCreated);
    let capacity = if mail_component::enabled() {
        json!({"apps":4,"cpuMillicores":2000,"memoryBytes":2147483648u64,"ephemeralBytes":1073741824u64,"volumeBytes":8589934592u64,"replicasPerApp":1,"concurrentDeployments":2})
    } else {
        json!({"apps":4,"cpuMillicores":1000,"memoryBytes":1073741824u64,"ephemeralBytes":1073741824u64,"volumeBytes":1073741824u64,"replicasPerApp":1,"concurrentDeployments":2})
    };
    okay(request(app,CONSOLE,"POST",&format!("/api/console/access/customers/{customer}/allowances"),console,
        json!({"expectedRevision":0,"idempotencyKey":"recovery-finite-allowance","hosting":capacity,"mailMessages":5})).await,"finite-allowance");
    phase(RecoveryPhase::AllowancesGranted);
    let offline = fixture.role("webdock_offline_migrate", "webdock_operator_test");
    let enrollment_input = path.join("agent-input.json");
    let enrollment_output = path.join("agent-output.json");
    let docker_source =
        std::env::var("WEBDOCK_RECOVERY_COMPONENT_DRIVER").as_deref() == Ok("docker");
    let mut reviewed_enrollment = json!({"platformID":"owned-recovery","agentID":"native-agent","name":"Owned isolated recovery node","country":"de","region":"eu","locationEvidence":"Disposable network-none local component, no production claim","transport":{"kind":"unix","uid":0}});
    if docker_source {
        reviewed_enrollment["runtimeKind"] = json!("docker");
        reviewed_enrollment["runtimeProfile"] = json!("local-recovery-fixture-v1");
    }
    private(&enrollment_input, &reviewed_enrollment);
    webdock_control_plane::plugin_bootstrap::enroll_agent(
        &HashMap::from([("DATABASE_URL_UNPOOLED".into(), offline.as_str().into())]),
        &target(&offline),
        &enrollment_input,
        &enrollment_output,
    )
    .await
    .unwrap();
    phase(RecoveryPhase::AgentEnrolled);
    let enrollment: Value =
        serde_json::from_slice(&std::fs::read(enrollment_output).unwrap()).unwrap();
    let cluster = enrollment["clusterID"].as_str().unwrap();
    let mut mail_foundation = if mail_component::enabled() {
        Some(
            mail_component::foundation(
                mail_dependencies.expect("actual Mail factory services"),
                hosting.clone(),
                cluster,
                customer,
                mail_component::prepare_outgoing_project(app, console).await,
            )
            .await,
        )
    } else {
        None
    };
    let control_path = path.join("control.sock");
    let control = webdock_agent_control::ControlPlane::new(
        auth.clone(),
        hosting.clone(),
        Some(webdock_agent_control::LocalBinding {
            installation: "owned-recovery".into(),
            agent_id: "native-agent".into(),
            uid: 0,
        }),
    )
    .await
    .unwrap();
    let control = if let Some(mail) = &mail_foundation {
        control.with_plugin_runtime(Arc::new(
            webdock_control_plane::plugins::installation::AgentBackend {
                host: mail.core.plugins.host.clone(),
            },
        ))
    } else {
        control
    };
    let socket = webdock_plugin_runtime::UnixSocket::bind(&control_path).unwrap();
    phase(RecoveryPhase::ControlSocketBound);
    let (stop_rpc, shutdown) = tokio::sync::oneshot::channel();
    let authority_state = Arc::new(app_component::ControlState::default());
    let server_state = authority_state.clone();
    let rpc = tokio::spawn(async move {
        if app_component::enabled() {
            app_component::serve(control, socket, server_state, async {
                let _ = shutdown.await;
            })
            .await;
        } else {
            control
                .serve_unix(socket, async {
                    let _ = shutdown.await;
                })
                .await
                .unwrap();
        }
    });
    let pool = if mail_component::enabled() {
        std::path::PathBuf::from("/owned/mail-data/native")
    } else {
        path.join("pool")
    };
    directory(&pool);
    let storage_config = path.join("storage.json");
    private(
        &storage_config,
        &json!({"root":pool,"node":std::env::var("HOSTNAME").unwrap(),"clusterID":cluster,"poolID":uuid::Uuid::new_v4(),"capacityBytes":if mail_component::enabled(){8589934592u64}else{1073741824u64},"reserveBytes":if mail_component::enabled(){268435456u64}else{67108864}}),
    );
    assert!(
        Command::new("/usr/local/bin/webdock-storage")
            .arg("--config")
            .arg(&storage_config)
            .arg("initialize")
            .status()
            .unwrap()
            .success()
    );
    phase(RecoveryPhase::StorageInitialized);
    let storage_socket = path.join("storage.sock");
    let mut helper = spawn(
        "/usr/local/bin/webdock-storage",
        &[
            "--config".as_ref(),
            storage_config.as_os_str(),
            "serve".as_ref(),
            "--socket".as_ref(),
            storage_socket.as_os_str(),
        ],
        path,
        "storage",
    );
    let state = path.join("agent-state");
    directory(&state);
    let config = path.join("agent.json");
    let mut agent_config = json!({"version":1,"installation":"owned-recovery","agentId":"native-agent","generation":1,"stateDirectory":state,"control":{"kind":"unix","path":control_path},"runtime":{"kind":"kubernetes","kubeconfig":"/etc/rancher/k3s/k3s.yaml","context":"default","helm":"/usr/bin/true","chart":"/owned/chart","chartSha256":std::env::var("WEBDOCK_RECOVERY_CHART_SHA256").unwrap(),"storageSocket":storage_socket},"workloads":false,"pluginRuntimeEnabled":false});
    if docker_source {
        agent_config["runtime"] = fixture_docker_runtime(&storage_socket);
    }
    if mail_component::enabled() {
        agent_config["workloads"] = json!(true);
        agent_config["pluginRuntimeEnabled"] = json!(true);
    }
    private(&config, &agent_config);
    let mut agent = spawn(
        "/usr/local/bin/webdock-agent",
        &["--config".as_ref(), config.as_os_str(), "run".as_ref()],
        path,
        "agent-first",
    );
    phase(RecoveryPhase::AgentSpawned);
    let mut capture = HeartbeatCapture {
        source: path.to_path_buf(),
        output: Path::new("/diagnostics").join(format!(
            "native-heartbeat-{}",
            uuid::Uuid::new_v4().simple()
        )),
        observe: || json!({"agent":diagnostic_child(agent.try_wait()),"helper":diagnostic_child(helper.try_wait()),"controlFinished":rpc.is_finished(),"controlResult":"not_consumed","heartbeatConfirmed":false}),
        armed: true,
    };
    let observed = wait_agent(owner, cluster, false).await;
    capture.disarm();
    drop(capture);
    assert_eq!(observed["observation"]["capabilities"]["storage"], true);
    phase(RecoveryPhase::AgentHeartbeatObserved);
    assert_eq!(
        observed["observation"]["runtimeKind"],
        if docker_source {
            "docker"
        } else {
            "kubernetes"
        }
    );
    okay(
        post(
            app,
            console,
            fixture_activate_command(cluster, &capacity, &observed),
        )
        .await,
        "activate-capacity",
    );
    phase(RecoveryPhase::CapacityActivated);

    okay(
        post(
            app,
            console,
            fixture_project_command(cluster, project, docker_source),
        )
        .await,
        "hosting-project",
    );
    phase(RecoveryPhase::HostingProjectConfigured);
    if let Some(mail) = mail_foundation.take() {
        mail_component::check(mail_component::Context {
            foundation: mail,
            app,
            console,
            fixture,
            owner,
            path,
            cluster,
            customer,
            project,
            agent: &mut agent,
            helper: &mut helper,
            agent_config: &mut agent_config,
            agent_config_path: &config,
            storage_config: &storage_config,
        })
        .await;
        stop(&mut agent).await;
        stop(&mut helper).await;
        let _ = stop_rpc.send(());
        rpc.await.unwrap();
        return;
    }
    let image = FIXTURE_NTFY_IMAGE;
    let prepared = okay(
        post(app, console, fixture_prepare_command(project)).await,
        "prepare-ntfy",
    );
    phase(RecoveryPhase::SourcePrepared);
    let source = prepared["appID"].as_str().unwrap();
    if app_component::enabled() {
        app_component::normal_guard_lock_compatibility(hosting, cluster, &enrollment, source).await;
    }
    let before = post(app, console, fixture_draft_policy_command(cluster, source)).await;
    assert!(
        expected_draft_policy_denial(before.status, &before.body),
        "unconfigured/draft request must reach the real domain denial, not parser/auth/transport failure; {}",
        draft_policy_denial_detail(before.status, &before.body)
    );
    phase(RecoveryPhase::DraftPolicyDenied);
    inspect_exact_fixture_image(image).await;
    let mut source_capture = HeartbeatCapture {
        source: path.to_path_buf(),
        output: Path::new("/diagnostics")
            .join(format!("native-source-{}", uuid::Uuid::new_v4().simple())),
        observe: || json!({"agent":diagnostic_child(agent.try_wait()),"helper":diagnostic_child(helper.try_wait()),"controlFinished":rpc.is_finished(),"controlResult":"not_consumed","heartbeatConfirmed":true,"phase":"source_deploy"}),
        armed: true,
    };
    okay(
        post(app, console, fixture_deploy_command(source, image)).await,
        "deploy-ntfy",
    );
    phase(RecoveryPhase::SourceDeployQueued);
    let mut public_source_state = "status=<unobserved> last_error=<unobserved>".to_owned();
    let source_result = tokio::time::timeout(Duration::from_secs(100), async {
        loop {
            let value = row(
                owner,
                "SELECT to_jsonb(a) AS value FROM webdock_auth.hosting_app a WHERE id=$1",
                vec![source.into()],
            )
            .await;
            public_source_state = public_app_failure(&value);
            if value["status"] == "ready" {
                break value;
            }
            assert!(
                value["last_error"].is_null(),
                "native deployment failed; {public_source_state}"
            );
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await;
    let source_row = source_result
        .unwrap_or_else(|_| panic!("actual native ntfy readiness; {public_source_state}"));
    assert!(source_row["observed_uid"].as_str().is_some());
    source_capture.disarm();
    drop(source_capture);
    eprintln!("recovery_component actual_native_source_ready=true source_uid_present=true");
    if byte_component::enabled() {
        byte_component::check(
            app_component::Context {
                hosting,
                enrollment: &enrollment,
                app,
                console,
                owner,
                path,
                cluster,
                source,
                source_row: &source_row,
                agent_config: &mut agent_config,
                agent_config_path: &config,
                storage_config: &storage_config,
                storage_socket: &storage_socket,
                control_path: &control_path,
                agent: &mut agent,
                helper: &mut helper,
                state: authority_state.clone(),
            },
            &fixture.container,
        )
        .await;
    } else if app_component::enabled() {
        app_component::check(app_component::Context {
            hosting,
            enrollment: &enrollment,
            app,
            console,
            owner,
            path,
            cluster,
            source,
            source_row: &source_row,
            agent_config: &mut agent_config,
            agent_config_path: &config,
            storage_config: &storage_config,
            storage_socket: &storage_socket,
            control_path: &control_path,
            agent: &mut agent,
            helper: &mut helper,
            state: authority_state,
        })
        .await;
    }
    stop(&mut agent).await;
    stop(&mut helper).await;
    let _ = stop_rpc.send(());
    rpc.await.unwrap();
}

// Only these owned fixture files may be inspected. Raw bodies never leave memory.
const DIAGNOSTIC_LOGS: [&str; 18] = [
    "storage-cold.stdout",
    "storage-cold.stderr",
    "storage-cold-bootstrap.stdout",
    "storage-cold-bootstrap.stderr",
    "agent-first.stdout",
    "agent-first.stderr",
    "storage.stdout",
    "storage.stderr",
    "agent-missing-journal.stdout",
    "agent-missing-journal.stderr",
    "agent-profile.stdout",
    "agent-profile.stderr",
    "storage-trusted.stdout",
    "storage-trusted.stderr",
    "agent-profile-trusted.stdout",
    "agent-profile-trusted.stderr",
    "agent-restarted.stdout",
    "agent-restarted.stderr",
];
const DIAGNOSTIC_CONFIGS: [&str; 4] = [
    "agent.json",
    "storage.json",
    "recovery.json",
    "recovery-trust.json",
];
const DIAGNOSTIC_LIMIT: u64 = 65536;
fn diagnostic_child(result: io::Result<Option<std::process::ExitStatus>>) -> Value {
    match result {
        Ok(Some(status)) => json!({"state":"exited","code":status.code(),"signal":status.signal()}),
        Ok(None) => json!({"state":"running"}),
        Err(error) => json!({"state":"unavailable","errorKind":format!("{:?}",error.kind())}),
    }
}
fn diagnostic_directory(path: &Path) -> io::Result<File> {
    let fd = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?;
    let file = File::from(fd);
    let metadata = file.metadata()?;
    if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o077 != 0 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(file)
}
fn diagnostic_file(directory: &File, name: &str) -> io::Result<File> {
    if !DIAGNOSTIC_LOGS.contains(&name) && !DIAGNOSTIC_CONFIGS.contains(&name) {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let fd = rustix::fs::openat(
        directory,
        name,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?;
    let file = File::from(fd);
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(file)
}
fn diagnostic_categories(raw: &[u8]) -> Vec<&'static str> {
    match raw.strip_suffix(b"\n").unwrap_or(raw) {
        b"Runtime resource ownership mismatch" => return vec!["ownership_mismatch_reported"],
        b"Runtime request failed; reconciliation required" => {
            return vec!["runtime_request_failed_reported"];
        }
        _ => {}
    }
    let text = String::from_utf8_lossy(raw).to_ascii_lowercase();
    let mut categories = Vec::new();
    for (needles, category) in [
        (
            &["authenticated control connection unavailable"][..],
            "control_connection_unavailable_reported",
        ),
        (
            &["durable agent identity unavailable"][..],
            "agent_identity_unavailable_reported",
        ),
        (
            &[
                "unknown field",
                "invalid configuration",
                "configuration error",
                "config error",
                "invalid type",
                "missing field",
                "unknown variant",
            ][..],
            "configuration_rejected",
        ),
        (
            &["connection refused", "connection reset", "transport error"][..],
            "transport_failure_reported",
        ),
        (&["permission denied"][..], "permission_denied_reported"),
        (
            &["no such file or directory"][..],
            "filesystem_entry_missing_reported",
        ),
        (
            &[
                "error while loading shared libraries",
                "symbol lookup error",
            ][..],
            "dynamic_loader_failure_reported",
        ),
        (
            &["unexpected argument", "unrecognized option", "usage:"][..],
            "argument_failure_reported",
        ),
        (&["heartbeat"][..], "heartbeat_mentioned"),
        (&["certificate", "tls"][..], "tls_mentioned"),
        (&["panic"][..], "panic_mentioned"),
        (&["error"][..], "error_mentioned"),
    ] {
        if needles.iter().any(|needle| text.contains(needle)) {
            categories.push(category);
        }
    }
    if !raw.is_empty() && categories.is_empty() {
        categories.push("unclassified_body_omitted");
    }
    categories
}
fn diagnostic_entry(directory: io::Result<&File>, name: &str, log: bool) -> Value {
    let read = (|| -> io::Result<Value> {
        let directory = directory?;
        let file = diagnostic_file(directory, name)?;
        let metadata = file.metadata()?;
        let mut raw = Vec::new();
        file.take(DIAGNOSTIC_LIMIT + 1).read_to_end(&mut raw)?;
        let truncated = raw.len() as u64 > DIAGNOSTIC_LIMIT || metadata.len() > DIAGNOSTIC_LIMIT;
        raw.truncate(DIAGNOSTIC_LIMIT as usize);
        let mut value = json!({"status":"captured","present":true,"uid":metadata.uid(),"gid":metadata.gid(),"mode":format!("{:04o}",metadata.mode()&0o7777),"fileBytesAtOpen":metadata.len()});
        if log {
            value["prefixBytes"] = json!(raw.len());
            value["truncated"] = json!(truncated);
            value["prefixSHA256"] = json!(format!("{:x}", Sha256::digest(&raw)));
            value["rawBodyRetained"] = json!(false);
            value["redaction"] =
                json!("all_raw_bytes_omitted_including_secrets_urls_tokens_and_keys");
            value["categories"] = json!(diagnostic_categories(&raw));
        } else if truncated {
            value["status"] = json!("oversized_body_not_retained");
        } else {
            value["bytes"] = json!(raw.len());
            value["sha256"] = json!(format!("{:x}", Sha256::digest(&raw)));
        }
        Ok(value)
    })();
    read.unwrap_or_else(|error|json!({"status":"unavailable","present":if error.kind()==io::ErrorKind::NotFound {Some(false)}else{None},"errorKind":format!("{:?}",error.kind()),"rawBodyRetained":false}))
}
fn write_diagnostic_json(path: &Path, value: &Value) -> io::Result<()> {
    let raw = serde_json::to_vec(value)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)?;
    file.write_all(&raw)?;
    file.sync_all()
}
fn diagnostic_boundaries(status: &Value) -> Vec<&'static str> {
    let mut observed = Vec::new();
    if status["startup"].is_object() {
        observed.push("startup_io_failure_observed");
    }
    if status["agent"]["state"] == "exited" {
        observed.push("agent_exit_observed");
    }
    if status["helper"]["state"] == "exited" {
        observed.push("storage_helper_exit_observed");
    }
    if status["controlFinished"] == true {
        observed.push("control_task_finished_before_heartbeat");
    }
    if status["agent"]["state"] == "unavailable" || status["helper"]["state"] == "unavailable" {
        observed.push("process_status_unavailable");
    }
    if status["agent"]["state"] == "running"
        && status["helper"]["state"] == "running"
        && status["controlFinished"] == false
    {
        observed.push("processes_running_heartbeat_unconfirmed");
    }
    observed
}
fn retain_heartbeat_diagnostics(source: &Path, output: &Path, status: Value) -> io::Result<()> {
    let parent = output.parent().ok_or(io::ErrorKind::InvalidInput)?;
    let _parent = diagnostic_directory(parent)?;
    std::fs::DirBuilder::new().mode(0o700).create(output)?;
    let source = diagnostic_directory(source);
    let mut receipt = json!({"scope":"owned_fixture_observations_only_no_readiness_or_authority","boundaries":diagnostic_boundaries(&status),"processes":status,"logs":{},"configs":{},"rawBodiesRetained":false});
    for name in DIAGNOSTIC_LOGS {
        let entry = diagnostic_entry(
            source.as_ref().map_err(|e| io::Error::from(e.kind())),
            name,
            true,
        );
        write_diagnostic_json(&output.join(format!("{name}.json")), &entry)?;
        receipt["logs"][name] = entry;
    }
    for name in DIAGNOSTIC_CONFIGS {
        receipt["configs"][name] = diagnostic_entry(
            source.as_ref().map_err(|e| io::Error::from(e.kind())),
            name,
            false,
        );
    }
    for name in [
        "core-signers",
        "operator-signers",
        "recovery-verification",
        "trusted-agent-staging",
        "recovery-peer",
        "recovery-peer/storage.sock",
    ] {
        receipt["trustMetadata"][name] =
            diagnostic_trust_metadata(source.as_ref().map_err(|e| io::Error::from(e.kind())), name);
    }
    write_diagnostic_json(&output.join("receipt.json"), &receipt)
}
fn diagnostic_trust_metadata(directory: io::Result<&File>, name: &str) -> Value {
    use std::os::unix::fs::FileTypeExt;
    let result = (|| -> io::Result<Value> {
        if ![
            "core-signers",
            "operator-signers",
            "recovery-verification",
            "trusted-agent-staging",
            "recovery-peer",
            "recovery-peer/storage.sock",
        ]
        .contains(&name)
        {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let directory = directory?;
        let peer;
        let (parent, entry) = if name == "recovery-peer/storage.sock" {
            peer = File::from(rustix::fs::openat(
                directory,
                "recovery-peer",
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::DIRECTORY
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            )?);
            (&peer, "storage.sock")
        } else {
            (directory, name)
        };
        let file = File::from(rustix::fs::openat(
            parent,
            entry,
            rustix::fs::OFlags::PATH | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )?);
        let m = file.metadata()?;
        Ok(
            json!({"status":"captured_metadata_only","type":if m.file_type().is_symlink(){"symlink"}else if m.is_dir(){"directory"}else if m.file_type().is_socket(){"socket"}else if m.is_file(){"regular"}else{"other"},"uid":m.uid(),"gid":m.gid(),"mode":format!("{:04o}",m.mode()&0o7777),"nlink":m.nlink(),"bytes":m.len()}),
        )
    })();
    result.unwrap_or_else(|error|json!({"status":"unavailable","errorKind":format!("{:?}",error.kind()),"rawBodyRetained":false}))
}
struct HeartbeatCapture<F: FnMut() -> Value> {
    source: std::path::PathBuf,
    output: std::path::PathBuf,
    observe: F,
    armed: bool,
}
impl<F: FnMut() -> Value> HeartbeatCapture<F> {
    fn disarm(&mut self) {
        self.armed = false;
    }
}
impl<F: FnMut() -> Value> Drop for HeartbeatCapture<F> {
    fn drop(&mut self) {
        if self.armed {
            let result = retain_heartbeat_diagnostics(&self.source, &self.output, (self.observe)());
            eprintln!(
                "recovery_component diagnostic_capture={}",
                if result.is_ok() {
                    "retained_categories_only"
                } else {
                    "unavailable"
                }
            );
        }
    }
}
#[cfg(test)]
mod recovery_diagnostics_tests {
    use super::*;
    use std::{
        io,
        os::unix::{fs::symlink, process::ExitStatusExt},
    };
    fn owned_temp() -> tempfile::TempDir {
        tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap()
    }
    fn write_owned(path: &Path, bytes: &[u8]) {
        std::fs::write(path, bytes).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    #[test]
    fn missing_files_and_failed_child_status_are_retained() {
        let source = owned_temp();
        let target = owned_temp();
        let output = target.path().join("capture");
        let status = json!({"agent":diagnostic_child(Err(io::ErrorKind::NotFound.into())),"helper":diagnostic_child(Ok(None)),"controlFinished":true});
        retain_heartbeat_diagnostics(source.path(), &output, status).unwrap();
        let receipt: Value =
            serde_json::from_slice(&std::fs::read(output.join("receipt.json")).unwrap()).unwrap();
        assert_eq!(receipt["processes"]["agent"]["state"], "unavailable");
        assert_eq!(receipt["processes"]["helper"]["state"], "running");
        assert_eq!(receipt["processes"]["controlFinished"], true);
        assert_eq!(receipt["configs"]["agent.json"]["status"], "unavailable");
        assert_eq!(
            receipt["logs"]["agent-first.stderr"]["status"],
            "unavailable"
        );
    }
    #[test]
    fn fixed_helper_and_agent_error_categories_do_not_retain_raw_material() {
        assert_eq!(
            diagnostic_categories(b"Runtime resource ownership mismatch\n"),
            vec!["ownership_mismatch_reported"]
        );
        assert_eq!(
            diagnostic_categories(b"Runtime request failed; reconciliation required\n"),
            vec!["runtime_request_failed_reported"]
        );
        assert_eq!(
            diagnostic_categories(b"Runtime resource ownership mismatch password=private"),
            vec!["unclassified_body_omitted"]
        );
    }
    #[test]
    fn trust_metadata_never_reads_signer_bodies_or_follows_foreign_paths() {
        let source = owned_temp();
        write_owned(
            &source.path().join("core-signers"),
            b"not-a-log-body-secret",
        );
        let directory = File::open(source.path()).unwrap();
        let value = diagnostic_trust_metadata(Ok(&directory), "core-signers");
        assert_eq!(value["type"], "regular");
        assert!(!value.to_string().contains("not-a-log-body-secret"));
        assert!(value.get("sha256").is_none());
        assert_eq!(
            diagnostic_trust_metadata(Ok(&directory), "secret.key")["status"],
            "unavailable"
        );
        let outside = owned_temp();
        symlink(outside.path(), source.path().join("recovery-peer")).unwrap();
        assert_eq!(
            diagnostic_trust_metadata(Ok(&directory), "recovery-peer")["type"],
            "symlink"
        );
        assert_eq!(
            diagnostic_trust_metadata(Ok(&directory), "recovery-peer/storage.sock")["status"],
            "unavailable"
        );
    }
    #[test]
    fn secrets_urls_keys_and_arbitrary_paths_never_leave_category_capture() {
        let source = owned_temp();
        let target = owned_temp();
        let output = target.path().join("capture");
        let secret = "fresh-fixture-secret-DO-NOT-COPY";
        let raw = format!(
            "config error unknown field; password={secret} token={secret} postgresql://user:{secret}@private.invalid/db Bearer {secret} -----BEGIN PRIVATE KEY-----\n{secret}\n-----END PRIVATE KEY-----"
        );
        write_owned(&source.path().join("agent-first.stderr"), raw.as_bytes());
        write_owned(
            &source.path().join("agent.json"),
            format!("{{\"password\":\"{secret}\"}}").as_bytes(),
        );
        write_owned(&source.path().join("secret.key"), secret.as_bytes());
        let outside = target.path().join("outside-secret");
        write_owned(&outside, secret.as_bytes());
        symlink(&outside, source.path().join("storage.stderr")).unwrap();
        retain_heartbeat_diagnostics(source.path(), &output, json!({})).unwrap();
        let mut captured = String::new();
        for entry in std::fs::read_dir(&output).unwrap() {
            captured.push_str(&std::fs::read_to_string(entry.unwrap().path()).unwrap());
        }
        for forbidden in [
            secret,
            "postgresql://",
            "Bearer ",
            "PRIVATE KEY",
            "password\"",
            "private.invalid",
        ] {
            assert!(!captured.contains(forbidden));
        }
        assert!(captured.contains("configuration_rejected"));
        assert!(!output.join("secret.key").exists());
        let dir = File::open(source.path()).unwrap();
        for forbidden in [
            "secret.key",
            "../outside-secret",
            "/etc/passwd",
            "agent-first.stderr/../secret.key",
        ] {
            assert_eq!(
                diagnostic_file(&dir, forbidden).unwrap_err().kind(),
                io::ErrorKind::PermissionDenied
            );
        }
    }
    #[test]
    fn panic_captures_before_source_cleanup_with_actual_exit_category() {
        let target = owned_temp();
        let output = target.path().join("capture");
        let source_path;
        {
            let source = owned_temp();
            source_path = source.path().to_owned();
            write_owned(
                &source.path().join("agent-first.stderr"),
                b"connection refused",
            );
            let _ = std::panic::catch_unwind(|| {
                let _capture = HeartbeatCapture {
                    source: source.path().to_owned(),
                    output: output.clone(),
                    observe: || json!({"agent":diagnostic_child(Ok(Some(std::process::ExitStatus::from_raw(17<<8))))}),
                    armed: true,
                };
                panic!("owned heartbeat test failure");
            });
        }
        assert!(!source_path.exists());
        let receipt: Value =
            serde_json::from_slice(&std::fs::read(output.join("receipt.json")).unwrap()).unwrap();
        assert_eq!(receipt["processes"]["agent"]["code"], 17);
        assert!(output.join("agent-first.stderr.json").exists());
    }
    #[test]
    fn success_disarms_capture_and_failed_capture_does_not_replace_original_panic() {
        let source = owned_temp();
        let target = owned_temp();
        let output = target.path().join("capture");
        {
            let mut capture = HeartbeatCapture {
                source: source.path().to_owned(),
                output: output.clone(),
                observe: || json!({}),
                armed: true,
            };
            capture.disarm();
        }
        assert!(!output.exists());
        write_owned(&output, b"blocked output");
        let panic = std::panic::catch_unwind(|| {
            let _capture = HeartbeatCapture {
                source: source.path().to_owned(),
                output: output.clone(),
                observe: || json!({}),
                armed: true,
            };
            panic!("original failure");
        })
        .unwrap_err();
        assert_eq!(panic.downcast_ref::<&str>(), Some(&"original failure"));
    }
    #[test]
    fn oversized_log_is_bounded_and_config_is_receipt_only() {
        let source = owned_temp();
        let target = owned_temp();
        let output = target.path().join("capture");
        write_owned(
            &source.path().join("agent-first.stdout"),
            &vec![b'x'; 65536 + 20],
        );
        write_owned(
            &source.path().join("storage.json"),
            b"{\"root\":\"private-path-must-not-leave\"}",
        );
        retain_heartbeat_diagnostics(source.path(), &output, json!({})).unwrap();
        let receipt: Value =
            serde_json::from_slice(&std::fs::read(output.join("receipt.json")).unwrap()).unwrap();
        assert_eq!(receipt["logs"]["agent-first.stdout"]["prefixBytes"], 65536);
        assert_eq!(receipt["logs"]["agent-first.stdout"]["truncated"], true);
        assert!(
            receipt["configs"]["storage.json"]["sha256"]
                .as_str()
                .is_some()
        );
        assert!(
            !std::fs::read_to_string(output.join("receipt.json"))
                .unwrap()
                .contains("private-path-must-not-leave")
        );
        assert!(!output.join("storage.json").exists());
    }
}

#[test]
fn recovery_diagnostics_distinguish_observations_without_raw_context() {
    let process = diagnostic_child(Ok(Some(std::process::ExitStatus::from_raw(9))));
    assert_eq!(process["signal"], 9);
    let status = json!({"agent":process,"helper":diagnostic_child(Ok(None)),"controlFinished":true,"startup":{"participant":"agent","errorKind":"NotFound"}});
    let categories = diagnostic_boundaries(&status);
    assert!(categories.contains(&"agent_exit_observed"));
    assert!(categories.contains(&"control_task_finished_before_heartbeat"));
    assert!(categories.contains(&"startup_io_failure_observed"));
    assert!(
        diagnostic_categories(b"Authenticated control connection unavailable")
            .contains(&"control_connection_unavailable_reported")
    );
    assert!(
        diagnostic_categories(b"configuration error unknown field token=private")
            .contains(&"configuration_rejected")
    );
    assert!(
        diagnostic_categories(b"transport error connection refused token=private")
            .contains(&"transport_failure_reported")
    );
}

fn fixture_docker_runtime(storage_socket: &Path) -> Value {
    json!({"kind":"docker","socket":"/var/run/docker.sock","initializerImage":null,"storageSocket":storage_socket})
}
#[test]
fn recovery_diagnostics_docker_source_uses_native_filesystem_socket_contract() {
    let runtime = fixture_docker_runtime(Path::new("/owned/storage.sock"));
    let socket = runtime["socket"].as_str().unwrap();
    assert!(
        socket.starts_with('/'),
        "native DockerAdapter requires an absolute filesystem socket before any I/O"
    );
    assert!(!socket.starts_with("unix:"));
    assert_eq!(socket, "/var/run/docker.sock");
    assert_eq!(runtime["kind"], "docker");
    assert_eq!(runtime["storageSocket"], "/owned/storage.sock");
    assert!(runtime["initializerImage"].is_null());
}

fn fixture_activate_command(cluster: &str, capacity: &Value, observed: &Value) -> Value {
    let observation = &observed["observation"];
    let nodes = observation["nodes"]
        .as_array()
        .expect("actual node observations");
    assert!(!nodes.is_empty());
    let resources=nodes.iter().map(|node|json!({"cpuMillicores":node["cpuMillicores"].as_u64().expect("reported node CPU"),"memoryBytes":node["memoryBytes"].as_u64().expect("reported node memory")})).collect::<Vec<_>>();
    let evidence = format!(
        "Owned isolated network-none/private-cgroup component: actual enrolled {} heartbeat, reported node resources {}, storage ready={} reportedBytes={}; finite requested cluster reservation {}. No production or physical-restore qualification.",
        observation["runtimeKind"]
            .as_str()
            .expect("actual runtime kind"),
        json!(resources),
        observation["capabilities"]["storage"]
            .as_bool()
            .expect("actual storage observation"),
        observation["capacity"]["volumeBytes"]
            .as_u64()
            .expect("reported storage capacity"),
        capacity
    );
    json!({"action":"clusters.activate","clusterID":cluster,"revision":1,"capacity":capacity,"verificationEvidence":evidence})
}
fn fixture_project_command(cluster: &str, project: &str, docker_source: bool) -> Value {
    json!({"action":"projects.create","projectID":project,"provider":if docker_source {"docker"} else {"k3s"},"clusterID":cluster,"mode":"selfservice","ownImages":true,"confirmSharedImages":true,"idempotencyKey":"recovery-project-hosting"})
}
fn fixture_ntfy_spec() -> Value {
    json!({"args":["serve"],"port":8080,"healthPath":"/v1/health","cpuMillicores":100,"memoryBytes":134217728,"ephemeralBytes":67108864,"replicas":1,"volumeBytes":67108864})
}
fn fixture_prepare_command(project: &str) -> Value {
    json!({"action":"apps.prepare","projectID":project,"name":"Owned ntfy","subscriptionRevision":1,"idempotencyKey":"ntfy-source-draft","spec":fixture_ntfy_spec(),"environment":[{"name":"NTFY_LISTEN_HTTP","value":":8080"},{"name":"NTFY_CACHE_FILE","value":"/data/cache.db"},{"name":"NTFY_AUTH_FILE","value":"/data/auth.db"},{"name":"NTFY_AUTH_DEFAULT_ACCESS","value":"deny-all"},{"name":"NTFY_ATTACHMENT_CACHE_DIR","value":"/data/attachments"},{"name":"NTFY_BASE_URL","value":"https://ntfy.owned.test"}]})
}
fn fixture_draft_policy_command(cluster: &str, source: &str) -> Value {
    json!({"action":"recovery.policies.grant","clusterID":cluster,"profile":"ntfy-sqlite-offline-v1","sourceAppID":source,"profileId":"ntfy-source","capacityBytes":67108864,"intervalSeconds":60,"idempotencyKey":"draft-source-denied"})
}
fn fixture_deploy_command(source: &str, image: &str) -> Value {
    let mut spec = fixture_ntfy_spec();
    spec["template"] = json!("custom");
    spec["image"] = json!(image);
    json!({"action":"apps.deploy","appID":source,"revision":1,"subscriptionRevision":1,"spec":spec,"idempotencyKey":"ntfy-source-deploy"})
}
fn public_app_failure(row: &Value) -> String {
    let status = row["status"]
        .as_str()
        .unwrap_or("<missing>")
        .chars()
        .take(64)
        .collect::<String>();
    let error = row["last_error"]
        .as_str()
        .unwrap_or("<none>")
        .chars()
        .take(256)
        .collect::<String>();
    format!("status={status} last_error={error}")
}
fn draft_policy_denial_detail(status: StatusCode, body: &Value) -> String {
    let code = body["error"]["code"]
        .as_str()
        .unwrap_or("<missing>")
        .chars()
        .take(64)
        .collect::<String>();
    let message = body["error"]["message"]
        .as_str()
        .unwrap_or("<missing>")
        .chars()
        .take(256)
        .collect::<String>();
    format!("status={} code={code} message={message}", status.as_u16())
}
fn expected_draft_policy_denial(status: StatusCode, body: &Value) -> bool {
    status == StatusCode::CONFLICT
        && body["error"]["message"]
            == "Current recovery policy, resource and enrolled agent are required."
}
#[test]
fn recovery_fixture_all_legacy_outgoing_commands_use_real_contract() {
    // Syntactic IDs/observations only, never a principal, row, Ready or authorization proof.
    let observation = json!({"observation":{"runtimeKind":"docker","nodes":[{"cpuMillicores":2000,"memoryBytes":4294967296u64}],"capabilities":{"storage":true},"capacity":{"volumeBytes":1073741824u64}}});
    let capacity = if mail_component::enabled() {
        json!({"apps":4,"cpuMillicores":2000,"memoryBytes":2147483648u64,"ephemeralBytes":1073741824u64,"volumeBytes":8589934592u64,"replicasPerApp":1,"concurrentDeployments":2})
    } else {
        json!({"apps":4,"cpuMillicores":1000,"memoryBytes":1073741824u64,"ephemeralBytes":1073741824u64,"volumeBytes":1073741824u64,"replicasPerApp":1,"concurrentDeployments":2})
    };
    let image = "binwiederhier/ntfy@sha256:6ef4b819f722fccdc036af611c4774cfdc2de821ab74fdd48bbf4c9d6f8973da";
    let mut invalid = Vec::new();
    for command in [
        fixture_activate_command("123", &capacity, &observation),
        fixture_project_command("123", "456", true),
        fixture_project_command("123", "456", false),
        fixture_prepare_command("456"),
        fixture_deploy_command("789", image),
    ] {
        if webdock_hosting::contracts::parse_command(command.clone()).is_err() {
            invalid.push(command["action"].as_str().unwrap().to_owned());
        }
    }
    assert!(
        invalid.is_empty(),
        "actual fixture outgoing DTOs rejected by real parser: {invalid:?}"
    );
}

#[test]
fn recovery_fixture_draft_denial_rejects_parser_auth_and_transport_failures() {
    let body = json!({"error":{"code":"hosting_error","message":"Current recovery policy, resource and enrolled agent are required."}});
    assert!(expected_draft_policy_denial(StatusCode::CONFLICT, &body));
    for status in [
        StatusCode::BAD_REQUEST,
        StatusCode::UNAUTHORIZED,
        StatusCode::FORBIDDEN,
        StatusCode::SERVICE_UNAVAILABLE,
    ] {
        assert!(!expected_draft_policy_denial(status, &body));
    }
    assert!(!expected_draft_policy_denial(
        StatusCode::BAD_REQUEST,
        &json!({"error":{"message":"Invalid recovery command."}})
    ));
    assert!(!expected_draft_policy_denial(
        StatusCode::CONFLICT,
        &json!({"error":{"message":"other failure"}})
    ));
    let command = fixture_draft_policy_command("123", "789");
    assert_eq!(command["profile"], "ntfy-sqlite-offline-v1");
    // Recovery dispatch uses its existing private serde DTO, not the legacy parser.
    assert!(webdock_hosting::contracts::parse_command(command).is_err());
}

#[test]
fn recovery_fixture_denial_detail_retains_only_public_error_fields() {
    let body = json!({"error":{"code":"hosting_error","message":"public failure","key":"secret-must-not-leave"},"session":"secret-must-not-leave","configuration":{"password":"secret-must-not-leave"}});
    let detail = draft_policy_denial_detail(StatusCode::SERVICE_UNAVAILABLE, &body);
    assert_eq!(
        detail,
        "status=503 code=hosting_error message=public failure"
    );
    assert!(!detail.contains("secret-must-not-leave"));
    let app = json!({"status":"failed","last_error":"public cause","spec":{"key":"secret-must-not-leave"},"environment":{"password":"secret-must-not-leave"},"desired":"secret-must-not-leave"});
    assert_eq!(
        public_app_failure(&app),
        "status=failed last_error=public cause"
    );
}

const FIXTURE_NTFY_IMAGE: &str =
    "binwiederhier/ntfy@sha256:6ef4b819f722fccdc036af611c4774cfdc2de821ab74fdd48bbf4c9d6f8973da";
fn bounded_fixture_image_metadata(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if object.len() != 3
        || !["Id", "RepoTags", "RepoDigests"]
            .iter()
            .all(|key| object.contains_key(*key))
    {
        return false;
    }
    if !value["Id"].as_str().is_some_and(|id| {
        id.strip_prefix("sha256:").is_some_and(|hash| {
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        })
    }) {
        return false;
    }
    ["RepoTags", "RepoDigests"].iter().all(|key| {
        value[key].is_null()
            || value[key].as_array().is_some_and(|rows| {
                rows.len() <= 16
                    && rows.iter().all(|row| {
                        row.as_str().is_some_and(|name| {
                            name.len() <= 512 && !name.chars().any(char::is_control)
                        })
                    })
            })
    })
}
fn exact_fixture_image_metadata(value: &Value, reference: &str) -> bool {
    reference == FIXTURE_NTFY_IMAGE
        && bounded_fixture_image_metadata(value)
        && value["RepoDigests"].as_array().is_some_and(|rows| {
            rows.iter().any(|row| {
                row.as_str().is_some_and(|digest| {
                    digest.split_once('@').is_some_and(|(_, hash)| {
                        Some(hash) == reference.split_once('@').map(|(_, hash)| hash)
                    })
                })
            })
        })
}
async fn inspect_exact_fixture_image(image: &str) {
    assert_eq!(
        image, FIXTURE_NTFY_IMAGE,
        "only the exact pinned public fixture image"
    );
    let mut command = tokio::process::Command::from(docker());
    command.args(["image","inspect","--format",r#"{"Id":{{json .Id}},"RepoTags":{{json .RepoTags}},"RepoDigests":{{json .RepoDigests}}}"#,image]).kill_on_drop(true);
    let inspected = tokio::time::timeout(Duration::from_secs(10), command.output()).await;
    let mut metadata = Value::Null;
    let mut exit = None;
    let mut reason = "inspect_unavailable";
    if let Ok(Ok(output)) = inspected {
        exit = output.status.code();
        if output.status.success() && output.stdout.len() <= 16384 {
            if let Ok(value) = serde_json::from_slice::<Value>(&output.stdout)
                && bounded_fixture_image_metadata(&value)
            {
                metadata = value;
                reason = "repo_digest_missing_or_mismatched";
            }
        } else if output.stderr.len() <= 1024
            && [
                format!("Error response from daemon: No such image: {image}"),
                format!("Error: No such image: {image}"),
            ]
            .iter()
            .any(|expected| String::from_utf8_lossy(&output.stderr).trim() == expected)
        {
            reason = "exact_digest_name_missing";
        }
    }
    let accepted = exact_fixture_image_metadata(&metadata, image);
    if accepted {
        reason = "exact_digest_verified";
    }
    let receipt = json!({"scope":"owned_inner_engine_readonly_pinned_public_image","expectedReference":image,"exitCode":exit,"category":reason,"metadata":metadata,"accepted":accepted});
    let path = Path::new("/diagnostics")
        .join(format!("ntfy-image-{}.json", uuid::Uuid::new_v4().simple()));
    write_diagnostic_json(&path, &receipt).expect("private bounded image metadata retention");
    eprintln!("recovery_component pinned_image_precondition={reason}");
    assert!(
        accepted,
        "exact pinned ntfy image precondition failed; category={reason} exit={exit:?}; bounded Id/RepoTags/RepoDigests retained, no image mutation/fallback/pull"
    );
}

#[test]
fn recovery_fixture_digest_metadata_requires_actual_matching_repo_digest() {
    let reference = "binwiederhier/ntfy@sha256:6ef4b819f722fccdc036af611c4774cfdc2de821ab74fdd48bbf4c9d6f8973da";
    let valid = json!({"Id":format!("sha256:{}","a".repeat(64)),"RepoTags":["binwiederhier/ntfy:latest"],"RepoDigests":[reference]});
    assert!(exact_fixture_image_metadata(&valid, reference));
    for invalid in [
        json!({"Id":valid["Id"],"RepoTags":["binwiederhier/ntfy:latest"],"RepoDigests":null}),
        json!({"Id":valid["Id"],"RepoTags":[],"RepoDigests":[format!("binwiederhier/ntfy@sha256:{}","b".repeat(64))]}),
        json!({"Id":valid["Id"],"RepoTags":[],"RepoDigests":[reference],"Config":{"Env":["secret"]}}),
    ] {
        assert!(!exact_fixture_image_metadata(&invalid, reference));
    }
}
