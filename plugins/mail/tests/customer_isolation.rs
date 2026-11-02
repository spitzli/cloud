//! Real disposable PostgreSQL; host and JMAP stand-ins never send mail or touch a cluster.
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio_postgres::NoTls;
use uuid::Uuid;
use webdock_mail::{Dns, Handler, Plugin, Settings, engine, seal::Sealer, store::Store, transport};
use webdock_plugin_protocol::{ErrorCode, InvocationClaims, PluginError};
use webdock_plugin_runtime::ActionHandler;

#[derive(Clone, Default)]
struct Host(Arc<Mutex<Vec<(String, Value)>>>);
#[tonic::async_trait]
impl engine::Workloads for Host {
    async fn call(&self, method: &str, input: Value) -> Result<Value, PluginError> {
        let setup = input["objects"].as_array().is_some_and(|objects| {
            objects
                .iter()
                .any(|o| o["metadata"]["name"] == "engine-recovery")
        });
        self.0.lock().unwrap().push((method.into(), input));
        if setup {
            return Err(PluginError::new(
                ErrorCode::Unavailable,
                "fixture engine offline",
            ));
        }
        Ok(if method == "apply" {
            json!({"revision":1})
        } else {
            json!({"appliedRevision":1,"observed":{"ready":true}})
        })
    }
}
struct NoDns;
#[tonic::async_trait]
impl Dns for NoDns {
    async fn txt(&self, _: &str) -> Vec<String> {
        vec![]
    }
}
struct Fixture {
    plugin: Arc<Plugin>,
    store: Store,
    admin: tokio_postgres::Client,
    url: url::Url,
    database: String,
    host: Host,
}
impl Fixture {
    async fn new() -> Self {
        let mut url = url::Url::parse(
            &std::env::var("WEBDOCK_MAIL_PLUGIN_TEST_ADMIN_URL")
                .expect("disposable PostgreSQL required"),
        )
        .unwrap();
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        let (admin, connection) = tokio_postgres::connect(url.as_str(), NoTls).await.unwrap();
        tokio::spawn(connection);
        let database = format!("mail_customer_test_{}", Uuid::new_v4().simple());
        admin
            .batch_execute(&format!("CREATE DATABASE {database}"))
            .await
            .unwrap();
        url.set_path(&database);
        let host = Host::default();
        let plugin = Self::plugin(&url, host.clone()).await;
        let store = Store::connect(url.as_str()).await.unwrap();
        Self {
            plugin,
            store,
            admin,
            url,
            database,
            host,
        }
    }
    async fn plugin(url: &url::Url, host: Host) -> Arc<Plugin> {
        Self::with_workloads(url, Box::new(host)).await
    }
    async fn with_workloads(url: &url::Url, workloads: Box<dyn engine::Workloads>) -> Arc<Plugin> {
        Plugin::new(
            Store::connect(url.as_str()).await.unwrap(),
            Sealer::new(&"27".repeat(32)).unwrap(),
            transport::Sender::new("http://127.0.0.1:9"),
            Box::new(NoDns),
            Settings {
                platform: transport::Platform::Smtp {
                    host: "127.0.0.1".into(),
                    port: 9,
                },
                trust_unverified_domains: true,
            },
            Some(
                engine::Engines::new(
                    workloads,
                    engine::EngineConfig {
                        image: format!("fixture.invalid/stalwart@sha256:{}", "a".repeat(64)),
                        domain: "mail.localhost".into(),
                        imap_port: 1993,
                        submission_port: 2465,
                        admin: engine::AdminRoute {
                            origin: "http://127.0.0.1:9".into(),
                            domain: "mail-admin.localhost".into(),
                            entry_points: vec!["web".into()],
                        },
                        issuer: None,
                    },
                )
                .unwrap(),
            ),
        )
    }
    async fn call(
        &self,
        project: &str,
        customer: &str,
        action: &str,
        input: Value,
    ) -> Result<Value, PluginError> {
        Handler(self.plugin.clone())
            .invoke(
                &InvocationClaims {
                    job: None,
                    v: 1,
                    id: Uuid::new_v4().to_string(),
                    plugin_id: "mail".into(),
                    generation: 1,
                    action: action.into(),
                    project_id: project.into(),
                    customer_id: customer.into(),
                    iat: 0,
                    exp: 0,
                },
                "",
                input,
                json!({}),
            )
            .await
    }
    async fn domain(&self, project: &str, customer: &str, domain: &str) -> String {
        self.call(
            project,
            customer,
            "domains.create",
            json!({"domain":domain}),
        )
        .await
        .unwrap()["resource"]["id"]
            .as_str()
            .unwrap()
            .into()
    }
    async fn settle(&self) {
        for _ in 0..100 {
            if self
                .store
                .pending_mailboxes()
                .await
                .unwrap()
                .iter()
                .all(|m| m.state != "creating")
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("fixture provisioning did not settle");
    }
    async fn cleanup(self) {
        drop(self.plugin);
        drop(self.store);
        self.admin
            .batch_execute(&format!("DROP DATABASE {} WITH (FORCE)", self.database))
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires disposable local PostgreSQL"]
async fn customer_engines_reuse_across_projects_and_deny_cross_customer_mailboxes() {
    let f = Fixture::new().await;
    let a = f.domain("11", "101", "a.example.test").await;
    let b = f.domain("12", "101", "b.example.test").await;
    let c = f.domain("21", "202", "c.example.test").await;
    assert!(
        f.store.find_engine("101").await.unwrap().is_none(),
        "outgoing domains allocate nothing"
    );
    let mut mailbox = String::new();
    let mut reservation = None;
    for (project, customer, domain) in [("11", "101", a), ("12", "101", b), ("21", "202", c)] {
        let created = f
            .call(
                project,
                customer,
                "mailboxes.create",
                json!({"domain_id":domain,"local_part":"hello"}),
            )
            .await
            .unwrap();
        mailbox = created["resource"]["id"].as_str().unwrap().into();
        f.settle().await;
        if customer == "101" {
            let current = f.store.find_engine(customer).await.unwrap().unwrap();
            if let Some((id, ref password)) = reservation {
                assert_eq!(current.id, id);
                assert_eq!(&current.admin_sealed, password);
            }
            reservation = Some((current.id, current.admin_sealed));
        }
    }
    let one = f.store.find_engine("101").await.unwrap().unwrap();
    let two = f.store.find_engine("202").await.unwrap().unwrap();
    assert_ne!(one.id, two.id);
    assert!(
        f.store.find_engine("11").await.unwrap().is_none(),
        "never project owned"
    );
    let calls = f.host.0.lock().unwrap().clone();
    let customers: Vec<_> = calls
        .iter()
        .filter(|(m, _)| m == "apply")
        .map(|(_, v)| {
            assert!(v.get("project").is_none());
            (v["customer"].clone(), v["unit"].clone())
        })
        .collect();
    assert!(customers.contains(&(json!("101"), json!("engine-c101"))));
    assert!(customers.contains(&(json!("202"), json!("engine-c202"))));
    assert!(!calls.iter().any(|(m, _)| m == "remove"));
    assert_eq!(
        f.call(
            "21",
            "101",
            "mailboxes.view",
            json!({"resource_id":mailbox})
        )
        .await
        .unwrap_err()
        .code,
        ErrorCode::PermissionDenied
    );
    assert_eq!(
        f.call(
            "11",
            "101",
            "mailboxes.view",
            json!({"resource_id":mailbox})
        )
        .await
        .unwrap_err()
        .code,
        ErrorCode::NotFound
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable local PostgreSQL"]
async fn suspension_and_restart_retain_customer_engine_credentials_and_volume() {
    let mut f = Fixture::new().await;
    let a = f.domain("11", "101", "a.example.test").await;
    f.call(
        "11",
        "101",
        "mailboxes.create",
        json!({"domain_id":a,"local_part":"hello"}),
    )
    .await
    .unwrap();
    f.settle().await;
    let before = f.store.find_engine("101").await.unwrap().unwrap();
    f.store.engine_enabled("101", false).await.unwrap();
    let pending = f.store.find_engine("101").await.unwrap().unwrap();
    assert!(!pending.desired_enabled);
    assert_eq!(
        pending.state, "suspending",
        "desired disablement alone is not observed suspension"
    );
    f.call("11", "101", "incoming.deactivate", json!({"resource_id":a}))
        .await
        .unwrap();
    let suspended = f.store.find_engine("101").await.unwrap().unwrap();
    assert!(!suspended.desired_enabled);
    assert_eq!(before.id, suspended.id);
    assert_eq!(before.admin_sealed, suspended.admin_sealed);
    let restart = Fixture::plugin(&f.url, f.host.clone()).await;
    f.host.0.lock().unwrap().clear();
    restart.resume().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(f.call("11","101","mail.send",json!({"resource_id":a,"from":"hello","to":"recipient@example.test","subject":"must not fall back","text":"fixture"})).await.unwrap_err().code, ErrorCode::CapabilityUnavailable);
    let calls = f.host.0.lock().unwrap().clone();
    assert!(
        calls.iter().all(|(m, v)| m == "observe"
            || (m == "apply"
                && v["objects"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|o| o["kind"] == "Deployment" && o["spec"]["replicas"] == 0))),
        "restart must not enable suspended incoming"
    );
    f.plugin = restart;
    f.call("11", "101", "incoming.activate", json!({"resource_id":a}))
        .await
        .unwrap();
    let after = f.store.find_engine("101").await.unwrap().unwrap();
    assert!(after.desired_enabled);
    assert_eq!(before.id, after.id);
    assert_eq!(before.admin_sealed, after.admin_sealed);
    let calls = f.host.0.lock().unwrap().clone();
    assert!(calls.iter().all(|(m, _)| m != "remove"));
    let suspend = calls
        .iter()
        .find(|(_, v)| {
            v["objects"].as_array().is_some_and(|objects| {
                objects
                    .iter()
                    .any(|o| o["kind"] == "Deployment" && o["spec"]["replicas"] == 0)
            })
        })
        .unwrap();
    assert!(suspend.1["objects"].as_array().unwrap().iter().any(|o| o["kind"]=="PersistentVolumeClaim" && o["metadata"]["name"]=="engine-data"));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable local PostgreSQL"]
async fn legacy_project_engines_are_quarantined_without_replacement() {
    let f = Fixture::new().await;
    let (db, connection) = tokio_postgres::connect(f.url.as_str(), NoTls)
        .await
        .unwrap();
    tokio::spawn(connection);
    db.execute(
        "INSERT INTO engines(project_id,id,admin_sealed,state) VALUES('11',$1,'\\x00','ready')",
        &[&Uuid::new_v4()],
    )
    .await
    .unwrap();
    let domain = f.domain("12", "101", "new.example.test").await;
    let view = f
        .call("12", "101", "domains.view", json!({"resource_id":domain}))
        .await
        .unwrap();
    assert_eq!(view["incoming"]["state"], "migration-required");
    assert_eq!(
        f.call(
            "12",
            "101",
            "mailboxes.create",
            json!({"domain_id":domain,"local_part":"hello"})
        )
        .await
        .unwrap_err()
        .code,
        ErrorCode::Conflict
    );
    assert!(f.store.find_engine("101").await.unwrap().is_none());
    f.plugin.resume().await.unwrap();
    assert!(f.host.0.lock().unwrap().is_empty());
    assert_eq!(
        db.query_one("SELECT count(*) FROM engines WHERE project_id='11'", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );
    drop(db);
    f.cleanup().await;
}

#[tokio::test]
async fn interrupted_mailbox_creation_reuses_existing_domain_account() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = Vec::new();
            let input = loop {
                let mut chunk = [0; 8192];
                let n = socket.read(&mut chunk).await.unwrap();
                assert_ne!(n, 0);
                buffer.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buffer);
                let Some((head, body)) = text.split_once("\r\n\r\n") else {
                    continue;
                };
                let length = head
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .map(|n| n.parse::<usize>().unwrap())
                    })
                    .unwrap();
                if body.len() >= length {
                    break serde_json::from_str::<Value>(&body[..length]).unwrap();
                }
            };
            let call = &input["methodCalls"][0];
            let method = call[0].as_str().unwrap();
            let result = match method {
                "x:Account/get" => {
                    json!({"list":[{"id":"account-7","name":"hello","domainId":"domain-1"},{"id":"account-8","name":"hello","domainId":"another-domain"}]})
                }
                "x:Account/set" if call[1].get("create").is_some() => {
                    json!({"notCreated":{"u":{"type":"alreadyExists"}}})
                }
                "x:Account/set" => {
                    assert!(call[1]["update"]["account-7"].is_object());
                    json!({"updated":{"account-7":null}})
                }
                _ => panic!("unexpected method"),
            };
            let body = json!({"methodResponses":[[method,result,"w"]]}).to_string();
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        }
    });
    let config = engine::EngineConfig {
        image: format!("fixture.invalid/stalwart@sha256:{}", "a".repeat(64)),
        domain: "mail.localhost".into(),
        imap_port: 1993,
        submission_port: 2465,
        admin: engine::AdminRoute {
            origin: format!("http://{address}"),
            domain: "mail-admin.localhost".into(),
            entry_points: vec!["web".into()],
        },
        issuer: None,
    };
    let engines = engine::Engines::new(Box::new(Host::default()), config).unwrap();
    assert_eq!(
        engines
            .client("101", "fixture-password")
            .create_mailbox("domain-1", "hello", "fixture-mailbox-password", 1024)
            .await
            .unwrap(),
        "account-7"
    );
    server.abort();
}

#[tokio::test]
async fn suspension_waits_for_observed_apply_revision_without_removing_volume() {
    struct Observations(Arc<Mutex<Vec<String>>>);
    #[tonic::async_trait]
    impl engine::Workloads for Observations {
        async fn call(&self, method: &str, input: Value) -> Result<Value, PluginError> {
            let mut calls = self.0.lock().unwrap();
            calls.push(method.into());
            Ok(match method {
                "apply" => {
                    assert_eq!(input["customer"], "101");
                    assert!(input.get("project").is_none());
                    json!({"revision":4})
                }
                "observe" => {
                    json!({"appliedRevision":if calls.len()==2 {3} else {4},"observed":{"ready":true}})
                }
                _ => panic!("suspension must retain its workload"),
            })
        }
    }
    let calls = Arc::new(Mutex::new(Vec::new()));
    let config = engine::EngineConfig {
        image: format!("fixture.invalid/stalwart@sha256:{}", "a".repeat(64)),
        domain: "mail.localhost".into(),
        imap_port: 1993,
        submission_port: 2465,
        admin: engine::AdminRoute {
            origin: "http://127.0.0.1:9".into(),
            domain: "mail-admin.localhost".into(),
            entry_points: vec!["web".into()],
        },
        issuer: None,
    };
    engine::Engines::new(Box::new(Observations(calls.clone())), config)
        .unwrap()
        .suspend("101")
        .await
        .unwrap();
    assert_eq!(*calls.lock().unwrap(), ["apply", "observe", "observe"]);
}

#[derive(Clone, Copy, PartialEq)]
enum Observation {
    Pending,
    Stopped,
    Failed,
}
#[derive(Clone)]
struct DelayedHost {
    entered: Arc<tokio::sync::Semaphore>,
    observed: tokio::sync::watch::Receiver<Observation>,
}
#[tonic::async_trait]
impl engine::Workloads for DelayedHost {
    async fn call(&self, method: &str, input: Value) -> Result<Value, PluginError> {
        match method {
            "apply" => {
                if input["objects"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|o| o["metadata"]["name"] == "engine-recovery")
                {
                    return Err(PluginError::new(
                        ErrorCode::Unavailable,
                        "fixture engine offline",
                    ));
                }
                assert!(
                    input["objects"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|o| o["kind"] == "Deployment" && o["spec"]["replicas"] == 0)
                );
                Ok(json!({"revision":4}))
            }
            "observe" => {
                self.entered.add_permits(1);
                let mut observed = self.observed.clone();
                while *observed.borrow_and_update() == Observation::Pending {
                    observed.changed().await.unwrap();
                }
                if *observed.borrow() == Observation::Failed {
                    return Err(PluginError::new(
                        ErrorCode::Unavailable,
                        "fixture observation failed",
                    ));
                }
                Ok(json!({"appliedRevision":4,"observed":{"ready":true}}))
            }
            _ => panic!("incoming lifecycle must never delete retained storage"),
        }
    }
}
async fn background_done(plugin: &Arc<Plugin>) {
    for _ in 0..300 {
        if Arc::strong_count(plugin) == 1 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("fixture background reconciliation did not finish");
}
async fn entered(host: &DelayedHost) {
    tokio::time::timeout(std::time::Duration::from_secs(3), host.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
}

#[tokio::test]
#[ignore = "requires disposable local PostgreSQL"]
async fn deactivation_returns_before_observation_and_restart_retains_pending_intent() {
    for observation in [Observation::Stopped, Observation::Failed] {
        let mut f = Fixture::new().await;
        let domain = f.domain("11", "101", "a.example.test").await;
        let (release, observed) = tokio::sync::watch::channel(Observation::Pending);
        let host = DelayedHost {
            entered: Arc::new(tokio::sync::Semaphore::new(0)),
            observed,
        };
        f.plugin = Fixture::with_workloads(&f.url, Box::new(host.clone())).await;
        f.call(
            "11",
            "101",
            "incoming.activate",
            json!({"resource_id":domain}),
        )
        .await
        .unwrap();
        background_done(&f.plugin).await;
        let before = f.store.find_engine("101").await.unwrap().unwrap();
        let action = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            f.call(
                "11",
                "101",
                "incoming.deactivate",
                json!({"resource_id":domain}),
            ),
        )
        .await;
        assert!(
            action.is_ok(),
            "deactivation must persist intent and return before host observation is released"
        );
        action.unwrap().unwrap();
        entered(&host).await;
        let pending = f.store.find_engine("101").await.unwrap().unwrap();
        assert!(!pending.desired_enabled);
        assert_eq!(pending.state, "suspending");
        let restart = Fixture::with_workloads(&f.url, Box::new(host.clone())).await;
        restart.resume().await.unwrap();
        entered(&host).await;
        assert_eq!(
            f.store.find_engine("101").await.unwrap().unwrap().state,
            "suspending"
        );
        release.send(observation).unwrap();
        background_done(&f.plugin).await;
        background_done(&restart).await;
        let state = f.store.find_engine("101").await.unwrap().unwrap();
        assert!(!state.desired_enabled);
        assert_eq!(
            state.state,
            if observation == Observation::Stopped {
                "suspended"
            } else {
                "suspending"
            }
        );
        assert_eq!(state.id, before.id);
        assert_eq!(state.admin_sealed, before.admin_sealed);
        if observation == Observation::Failed {
            release.send(Observation::Stopped).unwrap();
            restart.resume().await.unwrap();
            background_done(&restart).await;
            assert_eq!(
                f.store.find_engine("101").await.unwrap().unwrap().state,
                "suspended"
            );
        }
        f.plugin = restart;
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable local PostgreSQL"]
async fn pending_suspension_does_not_block_new_intent_or_another_customer() {
    let mut f = Fixture::new().await;
    let domain = f.domain("11", "101", "a.example.test").await;
    let other = f.domain("22", "202", "b.example.test").await;
    let (release, observed) = tokio::sync::watch::channel(Observation::Pending);
    let host = DelayedHost {
        entered: Arc::new(tokio::sync::Semaphore::new(0)),
        observed,
    };
    f.plugin = Fixture::with_workloads(&f.url, Box::new(host.clone())).await;
    f.call(
        "11",
        "101",
        "incoming.activate",
        json!({"resource_id":domain}),
    )
    .await
    .unwrap();
    background_done(&f.plugin).await;
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        f.call(
            "11",
            "101",
            "incoming.deactivate",
            json!({"resource_id":domain}),
        ),
    )
    .await
    .expect("deactivation must return before observation")
    .unwrap();
    entered(&host).await;
    for (project, customer, domain) in [("11", "101", domain), ("22", "202", other)] {
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            f.call(
                project,
                customer,
                "incoming.activate",
                json!({"resource_id":domain}),
            ),
        )
        .await
        .expect("intent admission must not wait behind background network I/O")
        .unwrap();
        let pending = f.store.find_engine(customer).await.unwrap().unwrap();
        assert!(pending.desired_enabled);
        assert_eq!(pending.state, "provisioning");
    }
    release.send(Observation::Failed).unwrap();
    background_done(&f.plugin).await;
    for customer in ["101", "202"] {
        let engine = f.store.find_engine(customer).await.unwrap().unwrap();
        assert!(engine.desired_enabled);
        assert_eq!(
            engine.state, "failed",
            "the offline fixture records actual activation failure, never stale suspension"
        );
    }
    f.cleanup().await;
}
