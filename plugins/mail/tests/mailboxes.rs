//! A real Stalwart CE engine on the local dev k3s (`WEBDOCK_WORKLOAD_TEST_KUBECONFIG`, context
//! `webdock-dev`) behind its Traefik, and plugin storage on a disposable local PostgreSQL
//! (`WEBDOCK_MAIL_PLUGIN_TEST_ADMIN_URL`, loopback). A stand-in for Core and the cluster's agent
//! applies the engine's objects with kubectl in a namespace of the test's own.
use serde_json::{Value, json};
use std::{process::Command, sync::Mutex, time::Duration};
use tokio_postgres::NoTls;
use webdock_mail::{Dns, Handler, Plugin, Settings, engine, seal::Sealer, store::Store, transport};
use webdock_plugin_protocol::{ErrorCode, InvocationClaims, PluginError};
use webdock_plugin_runtime::ActionHandler;

const IMAGE: &str = "docker.io/stalwartlabs/stalwart@sha256:74e5a7d55303ba525d939c6bf97ed4e010df7521f52d80afc22a815b66bd53f3";

const ORIGIN: &str = "http://127.0.0.1:55480";
fn config() -> engine::EngineConfig {
    engine::EngineConfig {
        image: IMAGE.into(),
        domain: "mail.localhost".into(),
        imap_port: 55993,
        submission_port: 55465,
        admin: engine::AdminRoute {
            origin: ORIGIN.into(),
            domain: "mail-admin.localhost".into(),
            entry_points: vec!["web".into()],
        },
        issuer: Some("webdock-dev-ca".into()),
    }
}

/// Applies, prunes and removes like the agent, in one namespace, through kubectl.
type Applied = std::collections::HashMap<String, Vec<(String, String)>>;
struct Kube {
    namespace: String,
    applied: std::sync::Arc<Mutex<Applied>>,
}
fn kubectl(args: &[&str], input: Option<&Value>) -> std::process::Output {
    let kubeconfig =
        std::env::var("WEBDOCK_WORKLOAD_TEST_KUBECONFIG").expect("local dev kubeconfig");
    let mut command = Command::new("kubectl");
    command
        .args(["--kubeconfig", &kubeconfig, "--context", "webdock-dev"])
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().unwrap();
    if let Some(input) = input {
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.to_string().as_bytes())
            .unwrap();
    }
    drop(child.stdin.take());
    child.wait_with_output().unwrap()
}
fn resource(object: &Value) -> (String, String) {
    let api = object["apiVersion"].as_str().unwrap();
    let kind = object["kind"].as_str().unwrap().to_ascii_lowercase();
    let kind = match api.rsplit_once('/') {
        Some((group, _)) => format!("{kind}.{group}"),
        None => kind,
    };
    (
        kind,
        object["metadata"]["name"].as_str().unwrap().to_owned(),
    )
}
#[tonic::async_trait]
impl engine::Workloads for Kube {
    async fn call(&self, method: &str, input: Value) -> Result<Value, PluginError> {
        let unit = input["unit"].as_str().expect("workload unit");
        let namespace = format!("{}-{unit}", self.namespace);
        let ns = namespace.as_str();
        Ok(match method {
            "apply" => {
                let exists = kubectl(&["get", "namespace", ns], None).status.success();
                if !exists {
                    let created = kubectl(&["create", "namespace", ns], None);
                    assert!(created.status.success(), "{created:?}");
                    kubectl(
                        &[
                            "label",
                            "namespace",
                            ns,
                            "pod-security.kubernetes.io/enforce=baseline",
                        ],
                        None,
                    );
                }
                let mut objects = input["objects"].as_array().unwrap().clone();
                for object in &mut objects {
                    object["metadata"]["namespace"] = json!(ns);
                }
                let out = kubectl(
                    &[
                        "apply",
                        "--server-side",
                        "--force-conflicts",
                        "--field-manager",
                        "webdock-test",
                        "-f",
                        "-",
                    ],
                    Some(&json!({"apiVersion": "v1", "kind": "List", "items": objects})),
                );
                assert!(
                    out.status.success(),
                    "{}",
                    String::from_utf8_lossy(&out.stderr)
                );
                let wanted: Vec<_> = objects.iter().map(resource).collect();
                let mut by_unit = self.applied.lock().unwrap();
                let applied = by_unit.entry(unit.to_owned()).or_default();
                for (kind, name) in applied.iter().filter(|r| !wanted.contains(r)) {
                    kubectl(
                        &["delete", "-n", ns, kind, name, "--ignore-not-found"],
                        None,
                    );
                }
                *applied = wanted;
                json!({"revision":1})
            }
            "remove" => {
                kubectl(
                    &[
                        "delete",
                        "namespace",
                        ns,
                        "--ignore-not-found",
                        "--wait=true",
                    ],
                    None,
                );
                self.applied.lock().unwrap().remove(unit);
                json!({})
            }
            _ if !kubectl(&["get", "namespace", ns], None).status.success() => {
                json!({"removed": true})
            }
            _ => {
                let out = kubectl(
                    &["get", "deployment", "engine", "-n", ns, "-o", "json"],
                    None,
                );
                let deployment: Value = serde_json::from_slice(&out.stdout).unwrap();
                let stopped = [
                    "replicas",
                    "readyReplicas",
                    "availableReplicas",
                    "updatedReplicas",
                    "currentReplicas",
                    "terminatingReplicas",
                ]
                .iter()
                .all(|key| deployment["status"][key].as_i64().unwrap_or(0) == 0);
                let observed = deployment["status"]["observedGeneration"]
                    .as_i64()
                    .unwrap_or(0)
                    >= deployment["metadata"]["generation"].as_i64().unwrap();
                json!({"appliedRevision":1,"observed":{"ready":stopped && observed}})
            }
        })
    }
}

fn admin_url() -> url::Url {
    let url = url::Url::parse(
        &std::env::var("WEBDOCK_MAIL_PLUGIN_TEST_ADMIN_URL").expect("disposable local server"),
    )
    .unwrap();
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    url
}
async fn admin() -> tokio_postgres::Client {
    let (client, connection) = tokio_postgres::connect(admin_url().as_str(), NoTls)
        .await
        .unwrap();
    tokio::spawn(connection);
    client
}
struct NoDns;
#[tonic::async_trait]
impl Dns for NoDns {
    async fn txt(&self, _: &str) -> Vec<String> {
        Vec::new()
    }
}
struct Fixture {
    handler: Handler,
    storage: String,
    project: String,
}
impl Fixture {
    async fn call(&self, project: &str, action: &str, input: Value) -> Result<Value, PluginError> {
        let claims = InvocationClaims {
            job: None,
            v: 1,
            id: uuid::Uuid::new_v4().to_string(),
            plugin_id: "mail".into(),
            generation: 1,
            action: action.into(),
            project_id: project.into(),
            customer_id: if project == "202" { "202" } else { "101" }.into(),
            iat: 0,
            exp: 0,
        };
        self.handler.invoke(&claims, "", input, json!({})).await
    }
    async fn ok(&self, action: &str, input: Value) -> Value {
        self.call(&self.project, action, input)
            .await
            .unwrap_or_else(|e| panic!("{action}: {e:?}"))
    }
    async fn code(&self, action: &str, input: Value) -> ErrorCode {
        self.call(&self.project, action, input)
            .await
            .unwrap_err()
            .code
    }
}
/// A JMAP session through the engine's route, as the mailbox's user.
async fn session(customer: &str, user: &str, password: &str) -> u16 {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("{ORIGIN}/jmap/session"))
        .header(
            reqwest::header::HOST,
            format!("c{customer}.mail-admin.localhost"),
        )
        .basic_auth(user, Some(password))
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

async fn wait_mailbox(f: &Fixture, project: &str, id: &str) -> Value {
    for _ in 0..600 {
        let view = f
            .call(project, "mailboxes.view", json!({"resource_id":id}))
            .await
            .unwrap();
        if view["state"] != "creating" {
            assert_eq!(view["state"], "ready", "{view}");
            return view;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("mailbox readiness timed out");
}

#[test]
fn engines_have_no_egress_and_drop_the_setup_administrator() {
    struct Nowhere;
    #[tonic::async_trait]
    impl engine::Workloads for Nowhere {
        async fn call(&self, _: &str, _: Value) -> Result<Value, PluginError> {
            unreachable!()
        }
    }
    let engines = engine::Engines::new(Box::new(Nowhere), config()).unwrap();
    let kinds = |objects: &Value| -> Vec<String> {
        objects
            .as_array()
            .unwrap()
            .iter()
            .map(|o| format!("{}/{}", o["kind"], o["metadata"]["name"]))
            .collect()
    };
    let setup = engines.objects("7", Some("pw"), "a");
    assert!(kinds(&setup).contains(&r#""Secret"/"engine-recovery""#.to_owned()));
    let ready = engines.objects("7", None, "b");
    assert!(!kinds(&ready).iter().any(|k| k.contains("Secret")));
    let deployment = ready
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["kind"] == "Deployment")
        .unwrap();
    assert_eq!(
        deployment["spec"]["template"]["spec"]["containers"][0]["envFrom"],
        json!([])
    );
    let policy = ready
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["kind"] == "NetworkPolicy")
        .unwrap();
    assert_eq!(
        (
            policy["spec"]["policyTypes"].clone(),
            policy["spec"]["egress"].clone()
        ),
        (json!(["Egress"]), json!([]))
    );
    let imaps = ready
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["metadata"]["name"] == "engine-imaps")
        .unwrap();
    assert_eq!(
        imaps["spec"]["routes"][0]["match"],
        "HostSNI(`c7.mail.localhost`)"
    );
    assert!(
        engine::Engines::new(
            Box::new(Nowhere),
            engine::EngineConfig {
                image: "stalwart:latest".into(),
                ..config()
            }
        )
        .is_err()
    );
}

#[tokio::test]
async fn unavailable_management_never_removes_retained_engine() {
    struct Offline(std::sync::Arc<Mutex<Vec<String>>>);
    #[tonic::async_trait]
    impl engine::Workloads for Offline {
        async fn call(&self, method: &str, _: Value) -> Result<Value, PluginError> {
            self.0.lock().unwrap().push(method.into());
            Err(PluginError::new(
                ErrorCode::Unavailable,
                "fixture unavailable",
            ))
        }
    }
    let calls = std::sync::Arc::new(Mutex::new(Vec::new()));
    let mut config = config();
    config.admin.origin = "http://127.0.0.1:9".into();
    let engines = engine::Engines::new(Box::new(Offline(calls.clone())), config).unwrap();
    assert!(engines.provision("7", "fixture-password").await.is_err());
    assert!(
        !calls.lock().unwrap().iter().any(|m| m == "remove"),
        "A health-check failure must not delete customer mail data"
    );
}

#[tokio::test]
#[ignore = "requires a disposable local PostgreSQL server and the local dev k3s"]
async fn mailbox_lifecycle() {
    let storage = format!("mailbox_test_{}", uuid::Uuid::new_v4().simple());
    admin()
        .await
        .batch_execute(&format!("CREATE DATABASE {storage}"))
        .await
        .unwrap();
    let mut url = admin_url();
    url.set_path(&storage);
    let project = format!("9{}", std::process::id());
    let namespace = format!("wd-mailtest-{project}");
    let applied = std::sync::Arc::new(Mutex::new(std::collections::HashMap::new()));
    let kube = || Kube {
        namespace: namespace.clone(),
        applied: applied.clone(),
    };
    let plugin = Plugin::new(
        Store::connect(url.as_str()).await.unwrap(),
        Sealer::new(&"57".repeat(32)).unwrap(),
        transport::Sender::new("http://127.0.0.1:9"),
        Box::new(NoDns),
        Settings {
            platform: transport::Platform::Smtp {
                host: "127.0.0.1".into(),
                port: 9,
            },
            trust_unverified_domains: true,
        },
        Some(engine::Engines::new(Box::new(kube()), config()).unwrap()),
    );
    let stored = Store::connect(url.as_str()).await.unwrap();
    let mut f = Fixture {
        handler: Handler(plugin),
        storage,
        project,
    };

    let domain = f
        .ok("domains.create", json!({"domain":"shop.example.test"}))
        .await["resource"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for bad in ["", "Anna Smith", "a..b", "-a"] {
        assert_eq!(
            f.code(
                "mailboxes.create",
                json!({"domain_id": domain, "local_part": bad})
            )
            .await,
            ErrorCode::InvalidInput,
            "{bad:?}"
        );
    }
    assert_eq!(
        f.call(
            "202",
            "mailboxes.create",
            json!({"domain_id": domain, "local_part": "x"})
        )
        .await
        .unwrap_err()
        .code,
        ErrorCode::NotFound,
        "another project's domain"
    );
    let created = f
        .ok(
            "mailboxes.create",
            json!({"domain_id": domain, "local_part": "Anna", "name": "Anna"}),
        )
        .await;
    assert_eq!(created["resource"]["name"], "anna@shop.example.test");
    let mailbox = created["resource"]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        f.code(
            "mailboxes.create",
            json!({"domain_id": domain, "local_part": "anna"})
        )
        .await,
        ErrorCode::Conflict
    );

    let mut view = Value::Null;
    for _ in 0..600 {
        view = f
            .ok("mailboxes.view", json!({"resource_id": mailbox}))
            .await;
        if view["state"] != "creating" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert_eq!(view["state"], "ready", "{view}");
    assert_eq!(
        f.call("202", "mailboxes.view", json!({"resource_id": mailbox}))
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    let password = f
        .ok("mailboxes.credentials", json!({"resource_id": mailbox}))
        .await["value"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        session("101", "anna@shop.example.test", &password).await,
        200
    );
    assert_eq!(session("101", "anna@shop.example.test", "wrong").await, 401);
    // Another project of the same customer uses the same retained engine.
    let second_domain = f
        .call(
            "303",
            "domains.create",
            json!({"domain":"second.example.test"}),
        )
        .await
        .unwrap()["resource"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let second = f
        .call(
            "303",
            "mailboxes.create",
            json!({"domain_id":second_domain,"local_part":"bob"}),
        )
        .await
        .unwrap()["resource"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let second_view = wait_mailbox(&f, "303", &second).await;
    assert_eq!(second_view["host"], view["host"]);
    assert_eq!(
        f.call("303", "mailboxes.view", json!({"resource_id":mailbox}))
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    let before = stored.find_engine("101").await.unwrap().unwrap();

    // Different customers have separate CE installations and credentials cannot cross them.
    let other_domain = f
        .call(
            "202",
            "domains.create",
            json!({"domain":"other.example.test"}),
        )
        .await
        .unwrap()["resource"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let other = f
        .call(
            "202",
            "mailboxes.create",
            json!({"domain_id":other_domain,"local_part":"eve"}),
        )
        .await
        .unwrap()["resource"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let other_view = wait_mailbox(&f, "202", &other).await;
    assert_ne!(other_view["host"], view["host"]);
    assert_eq!(
        session("202", "anna@shop.example.test", &password).await,
        401
    );
    let other_password = f
        .call("202", "mailboxes.credentials", json!({"resource_id":other}))
        .await
        .unwrap()["value"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        session("202", "eve@other.example.test", &other_password).await,
        200
    );

    // Suspension retains the PVC UID, encrypted credentials and reservation across runtime restart.
    let ns = format!("{namespace}-engine-c101");
    let pvc = || {
        kubectl(
            &[
                "get",
                "pvc",
                "engine-data",
                "-n",
                &ns,
                "-o",
                "jsonpath={.metadata.uid}",
            ],
            None,
        )
        .stdout
    };
    let retained = pvc();
    assert!(!retained.is_empty());
    f.ok("incoming.deactivate", json!({"resource_id":domain}))
        .await;
    assert_eq!(pvc(), retained);
    for _ in 0..600 {
        if stored.find_engine("101").await.unwrap().unwrap().state == "suspended" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert_eq!(
        stored.find_engine("101").await.unwrap().unwrap().state,
        "suspended"
    );
    // Crash after Stalwart account creation, before the ready row was recorded.
    let (db, connection) = tokio_postgres::connect(url.as_str(), NoTls).await.unwrap();
    tokio::spawn(connection);
    db.execute(
        "UPDATE mailboxes SET state='creating',account_id=NULL WHERE id=$1",
        &[&uuid::Uuid::parse_str(&mailbox).unwrap()],
    )
    .await
    .unwrap();
    drop(db);
    f.handler = Handler(Plugin::new(
        Store::connect(url.as_str()).await.unwrap(),
        Sealer::new(&"57".repeat(32)).unwrap(),
        transport::Sender::new("http://127.0.0.1:9"),
        Box::new(NoDns),
        Settings {
            platform: transport::Platform::Smtp {
                host: "127.0.0.1".into(),
                port: 9,
            },
            trust_unverified_domains: true,
        },
        Some(engine::Engines::new(Box::new(kube()), config()).unwrap()),
    ));
    f.handler.0.resume().await.unwrap();
    assert!(
        !stored
            .find_engine("101")
            .await
            .unwrap()
            .unwrap()
            .desired_enabled
    );
    f.call(
        "303",
        "incoming.activate",
        json!({"resource_id":second_domain}),
    )
    .await
    .unwrap();
    for _ in 0..600 {
        if matches!(
            stored
                .find_engine("101")
                .await
                .unwrap()
                .unwrap()
                .state
                .as_str(),
            "ready" | "failed"
        ) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let after = stored.find_engine("101").await.unwrap().unwrap();
    assert_eq!(after.state, "ready");
    assert_eq!(after.id, before.id);
    assert_eq!(after.admin_sealed, before.admin_sealed);
    assert_eq!(pvc(), retained);
    assert_eq!(
        wait_mailbox(&f, &f.project, &mailbox).await["state"],
        "ready"
    );
    assert_eq!(
        session("101", "anna@shop.example.test", &password).await,
        200
    );

    // Mail clients reach IMAPS by server name and can verify the issued certificate.
    let kubeconfig = std::env::var("WEBDOCK_WORKLOAD_TEST_KUBECONFIG").unwrap();
    let ca = std::path::Path::new(&kubeconfig)
        .parent()
        .unwrap()
        .join("../ca/ca.crt");
    let host = view["host"].as_str().unwrap();
    let handshake = Command::new("openssl")
        .args([
            "s_client",
            "-connect",
            "127.0.0.1:55993",
            "-servername",
            host,
            "-verify_hostname",
            host,
            "-verify_return_error",
            "-CAfile",
        ])
        .arg(&ca)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&handshake.stdout).contains("Verify return code: 0 (ok)"),
        "{}",
        String::from_utf8_lossy(&handshake.stderr)
    );

    // The domain shows its DKIM record once mailboxes use it, and cannot be removed under them.
    let domain_view = f.ok("domains.view", json!({"resource_id": domain})).await;
    assert!(
        domain_view["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["name"]
                .as_str()
                .unwrap()
                .contains("._domainkey.shop.example.test")),
        "{domain_view}"
    );
    assert_eq!(domain_view["mailboxes"], json!(["anna@shop.example.test"]));
    assert_eq!(
        f.code("domains.delete", json!({"resource_id": domain}))
            .await,
        ErrorCode::Conflict
    );

    f.ok("mailboxes.rotate", json!({"resource_id": mailbox}))
        .await;
    assert_eq!(
        session("101", "anna@shop.example.test", &password).await,
        401
    );
    let rotated = f
        .ok("mailboxes.credentials", json!({"resource_id": mailbox}))
        .await["value"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        session("101", "anna@shop.example.test", &rotated).await,
        200
    );

    f.ok("mailboxes.delete", json!({"resource_id": mailbox}))
        .await;
    assert_eq!(
        session("101", "anna@shop.example.test", &rotated).await,
        401
    );
    f.ok("mailboxes.delete", json!({"resource_id": mailbox}))
        .await;
    f.ok("domains.delete", json!({"resource_id": domain})).await;

    let remaining_password = f
        .call(
            "303",
            "mailboxes.credentials",
            json!({"resource_id":second}),
        )
        .await
        .unwrap()["value"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        session("101", "bob@second.example.test", &remaining_password).await,
        200
    );
    assert_eq!(
        stored.find_engine("101").await.unwrap().unwrap().id,
        before.id
    );
    f.call("303", "mailboxes.delete", json!({"resource_id":second}))
        .await
        .unwrap();
    f.call(
        "303",
        "domains.delete",
        json!({"resource_id":second_domain}),
    )
    .await
    .unwrap();
    engine::Engines::new(Box::new(kube()), config())
        .unwrap()
        .destroy("202")
        .await
        .unwrap();
    engine::Engines::new(Box::new(kube()), config())
        .unwrap()
        .destroy("101")
        .await
        .unwrap();
    drop(f.handler);
    drop(stored);
    admin()
        .await
        .batch_execute(&format!("DROP DATABASE {} WITH (FORCE)", f.storage))
        .await
        .unwrap();
}
