//! Controlled CE boundary + real owned TLS PostgreSQL; no Core or cluster authority.
use super::*;
use crate::store::Store;
use axum::{
    Router, body::Body, extract::State, http::StatusCode, response::IntoResponse, routing::post,
};
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tokio::sync::Semaphore;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Success,
    LostResponse,
    Timeout,
    Http400,
    ApplyLost,
    SuspendLost,
    PauseApply,
}
struct Provider {
    outcome: Outcome,
    accepted: Semaphore,
    release: Semaphore,
    calls: AtomicUsize,
    accounts: AtomicUsize,
}
impl Provider {
    async fn boundary(&self) {
        self.accepted.add_permits(1);
        self.release.acquire().await.unwrap().forget();
    }
    async fn accepted(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.accepted.acquire())
            .await
            .expect("provider_accept_deadline")
            .unwrap()
            .forget();
    }
}
#[derive(Clone)]
struct Host(Arc<Provider>);
#[tonic::async_trait]
impl engine::Workloads for Host {
    async fn call(&self, method: &str, input: Value) -> Result<Value> {
        self.0.calls.fetch_add(1, Ordering::SeqCst);
        match method {
            "apply" => {
                assert!(matches!(input["customer"].as_str(), Some("101" | "202")));
                assert_eq!(
                    input["unit"],
                    engine::unit(input["customer"].as_str().unwrap())
                );
                if matches!(
                    self.0.outcome,
                    Outcome::ApplyLost | Outcome::SuspendLost | Outcome::PauseApply
                ) {
                    self.0.boundary().await;
                    if self.0.outcome != Outcome::PauseApply {
                        return Err(unavailable("fixture", "accepted_response_lost"));
                    }
                }
                Ok(json!({"revision":1}))
            }
            "observe" => Ok(json!({"appliedRevision":1,"observed":{"ready":true}})),
            _ => panic!("unexpected_fixture_workload_method"),
        }
    }
}
async fn jmap(
    State(p): State<Arc<Provider>>,
    headers: axum::http::HeaderMap,
    axum::Json(input): axum::Json<Value>,
) -> axum::response::Response {
    p.calls.fetch_add(1, Ordering::SeqCst);
    // The removed bootstrap administrator must give a definitive 401.
    if headers
        .get("authorization")
        .is_some_and(|v| v.as_bytes().starts_with(b"Basic YWRtaW46"))
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let call = &input["methodCalls"][0];
    let method = call[0].as_str().unwrap();
    let value = match method {
        "x:Domain/get" => json!({"list":[{"id":"d","name":"fixture.example.test"}]}),
        "x:Account/get" => json!({"list":[{"id":"admin","name":"webdock-admin"}]}),
        "x:Account/set" if call[1].get("create").is_some() => {
            assert_eq!(call[1]["create"]["u"]["domainId"], "d");
            p.accounts.fetch_add(1, Ordering::SeqCst); // Effect accepted before the response boundary.
            p.boundary().await;
            match p.outcome {
                Outcome::Timeout => {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                }
                Outcome::LostResponse => {
                    let stream = tokio_stream::iter([Err::<&'static str, _>(
                        std::io::Error::other("fixture_response_lost"),
                    )]);
                    return axum::response::Response::new(Body::from_stream(stream));
                }
                Outcome::Http400 => return StatusCode::BAD_REQUEST.into_response(),
                _ => {}
            }
            json!({"created":{"u":{"id":"account"}}})
        }
        "x:Account/set" => json!({"updated":{"admin":null}}),
        "x:NetworkListener/get" => {
            json!({"list":[{"id":"http","name":"http"},{"id":"imaps","name":"imaps"},{"id":"submissions","name":"submissions"}]})
        }
        "x:Certificate/get" => json!({"list":[{"id":"certificate"}]}),
        "x:SystemSettings/set" => json!({"updated":{"singleton":null}}),
        _ => panic!("unexpected_fixture_jmap_method"),
    };
    axum::Json(json!({"methodResponses":[[method,value,"w"]]})).into_response()
}
struct NoDns;
#[tonic::async_trait]
impl Dns for NoDns {
    async fn txt(&self, _: &str) -> Vec<String> {
        Vec::new()
    }
}
struct Fixture {
    plugin: Arc<Plugin>,
    db: tokio_postgres::Client,
    provider: Arc<Provider>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    async fn new(outcome: Outcome) -> Self {
        let url =
            std::env::var("WEBDOCK_MAIL_RECOVERY_TEST_URL").expect("owned_fixture_url_required");
        let ca = std::fs::read(
            std::env::var("WEBDOCK_MAIL_RECOVERY_TEST_CA").expect("owned_fixture_ca_required"),
        )
        .unwrap();
        let store = Store::connect_tls(&url, &ca)
            .await
            .expect("owned_store_connect");
        let db = webdock_plugin_kit::postgres::connect(&url, Some(&ca))
            .await
            .expect("owned_sql_connect");
        let ownership = db.query_one("SELECT current_user::text,pg_get_userbyid(datdba)::text,(SELECT ssl FROM pg_stat_ssl WHERE pid=pg_backend_pid()) FROM pg_database WHERE datname=current_database()", &[]).await.unwrap();
        assert_eq!(ownership.get::<_, String>(0), "mail_owner");
        assert_eq!(ownership.get::<_, String>(1), "mail_owner");
        assert!(ownership.get::<_, bool>(2));
        let provider = Arc::new(Provider {
            outcome,
            accepted: Semaphore::new(0),
            release: Semaphore::new(0),
            calls: AtomicUsize::new(0),
            accounts: AtomicUsize::new(0),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/jmap", post(jmap))
            .with_state(provider.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let engines = engine::Engines::new(
            Box::new(Host(provider.clone())),
            engine::EngineConfig {
                image: format!("fixture.invalid/stalwart@sha256:{}", "a".repeat(64)),
                domain: "fixture.localhost".into(),
                imap_port: 1993,
                submission_port: 2465,
                admin: engine::AdminRoute {
                    origin,
                    domain: "admin.localhost".into(),
                    entry_points: vec!["web".into()],
                },
                issuer: Some("fixture".into()),
            },
        )
        .unwrap();
        let platform = transport::Platform::Smtp {
            host: "127.0.0.1".into(),
            port: 9,
        };
        let plugin = Plugin::new(
            store,
            seal::Sealer::new(&"24".repeat(32)).unwrap(),
            transport::Sender::new("http://127.0.0.1:9"),
            Box::new(NoDns),
            Settings {
                platform,
                trust_unverified_domains: true,
            },
            Some(engines),
        );
        Self {
            plugin,
            db,
            provider,
            server,
        }
    }
    async fn mailbox(&self, customer: &str) -> store::Mailbox {
        self.plugin.engine_password(customer).await.unwrap();
        let domain = store::Domain {
            id: Uuid::new_v4(),
            domain: "fixture.example.test".into(),
            token: "fixture".into(),
            verified: false,
        };
        self.plugin
            .store
            .add_domain(customer, &domain)
            .await
            .unwrap();
        let id = Uuid::new_v4();
        let mailbox = store::Mailbox {
            id,
            project_id: customer.into(),
            customer_id: customer.into(),
            domain_id: domain.id,
            domain: domain.domain,
            local_part: "hello".into(),
            display_name: "Fixture".into(),
            password_sealed: self.plugin.sealer.seal(&id, "fixture-password"),
            account_id: None,
            quota_mb: 1,
            state: "creating".into(),
            error: None,
            created_at: String::new(),
        };
        assert!(self.plugin.store.add_mailbox(&mailbox).await.unwrap());
        mailbox
    }
    async fn count(&self) -> i64 {
        self.db
            .query_one("SELECT count(*) FROM mail_recovery_effect", &[])
            .await
            .unwrap()
            .get(0)
    }
    async fn settle(&self) {
        drop(
            tokio::time::timeout(Duration::from_secs(15), self.plugin.provisioning.lock())
                .await
                .expect("background_completion_deadline"),
        );
    }
    async fn closed(&self) {
        assert_eq!(
            self.count().await,
            1,
            "ambiguous_provider_outcome_lost_durable_custody"
        );
        let url = std::env::var("WEBDOCK_MAIL_RECOVERY_TEST_URL").unwrap();
        let ca = std::fs::read(std::env::var("WEBDOCK_MAIL_RECOVERY_TEST_CA").unwrap()).unwrap();
        let store = Store::connect_tls(&url, &ca).await.unwrap();
        let state = store.recovery_state().await.unwrap();
        let operation = Uuid::new_v4();
        assert!(
            store
                .fence_recovery(state.epoch, operation, 1)
                .await
                .is_err(),
            "uncertain_provider_was_acknowledged_drained"
        );
        assert_eq!(store.recovery_state().await.unwrap().state, "fenced");
        let revision = self
            .db
            .query_one(
                "SELECT revision FROM mail_recovery_control WHERE singleton",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        let fence = crate::recovery::RecoveryFence {
            epoch: state.epoch,
            operation,
            generation: 1,
            revision,
        };
        assert!(
            store.resume_recovery(&fence).await.is_err(),
            "uncertain_source_resumed"
        );
        let reopened = Store::connect_tls(&url, &ca).await.unwrap();
        assert!(reopened.recovery_guard().await.is_err());
        assert!(
            reopened
                .fence_recovery(state.epoch, operation, 1)
                .await
                .is_err()
        );
        let restored_epoch = reopened.quarantine_recovery(&fence).await.unwrap();
        assert_ne!(restored_epoch, state.epoch);
        assert!(reopened.resume_recovery(&fence).await.is_err());
        assert!(reopened.recovery_effect().await.is_err());
        assert_eq!(self.count().await, 1);
        assert_eq!(
            reopened.recovery_state().await.unwrap().state,
            "quarantined"
        );
    }
}

#[tokio::test]
#[ignore = "owned TLS PostgreSQL; plugins/mail/tests/fixtures/mailbox-recovery.py"]
async fn mailbox_unknown_response_retains_custody() {
    let outcome = match std::env::var("WEBDOCK_MAIL_FIXTURE_OUTCOME")
        .unwrap()
        .as_str()
    {
        "lost" => Outcome::LostResponse,
        "timeout" => Outcome::Timeout,
        "http400" => Outcome::Http400,
        _ => panic!("unknown_fixture_outcome"),
    };
    let f = Fixture::new(outcome).await;
    let mailbox = f.mailbox("101").await;
    f.plugin.provision(mailbox.clone());
    f.provider.accepted().await;
    assert_eq!(f.count().await, 1);
    f.provider.release.add_permits(1);
    f.settle().await;
    assert_eq!(f.provider.accounts.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.plugin
            .store
            .mailbox("101", &mailbox.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        "failed"
    );
    f.closed().await;
}

#[tokio::test]
#[ignore = "owned TLS PostgreSQL; plugins/mail/tests/fixtures/mailbox-recovery.py"]
async fn reconciliation_partial_completion_retains_custody() {
    let suspend = std::env::var("WEBDOCK_MAIL_FIXTURE_OUTCOME").unwrap() == "suspend";
    let f = Fixture::new(if suspend {
        Outcome::SuspendLost
    } else {
        Outcome::ApplyLost
    })
    .await;
    f.plugin.engine_password("101").await.unwrap();
    if suspend {
        f.plugin.store.engine_enabled("101", false).await.unwrap();
    }
    f.plugin.reconcile("101".into());
    f.provider.accepted().await;
    assert_eq!(f.count().await, 1);
    f.provider.release.add_permits(1);
    f.settle().await;
    assert_eq!(
        f.plugin
            .store
            .find_engine("101")
            .await
            .unwrap()
            .unwrap()
            .state,
        if suspend { "suspending" } else { "failed" }
    );
    f.closed().await;
}

#[tokio::test]
#[ignore = "owned TLS PostgreSQL; plugins/mail/tests/fixtures/mailbox-recovery.py"]
async fn mailbox_success_without_metadata_persistence_retains_custody() {
    let f = Fixture::new(Outcome::Success).await;
    let mailbox = f.mailbox("101").await;
    let refusal = if std::env::var("WEBDOCK_MAIL_FIXTURE_OUTCOME").unwrap() == "skip" {
        "RETURN NULL;"
    } else {
        "RAISE EXCEPTION 'fixture_metadata_failure';"
    };
    f.db.batch_execute(&format!("CREATE FUNCTION fixture_reject_ready() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.state='ready' THEN {refusal} END IF; RETURN NEW; END $$; CREATE TRIGGER fixture_reject_ready BEFORE UPDATE ON mailboxes FOR EACH ROW EXECUTE FUNCTION fixture_reject_ready()")).await.unwrap();
    f.plugin.provision(mailbox.clone());
    f.provider.accepted().await;
    f.provider.release.add_permits(1);
    f.settle().await;
    assert_eq!(f.provider.accounts.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.plugin
            .store
            .mailbox("101", &mailbox.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        "creating"
    );
    f.closed().await;
}

#[tokio::test]
#[ignore = "owned TLS PostgreSQL; plugins/mail/tests/fixtures/mailbox-recovery.py"]
async fn definite_success_and_never_started_release_only_their_own_custody() {
    let f = Fixture::new(Outcome::Success).await;
    let unrelated = f.plugin.store.recovery_effect().await.unwrap();
    let mailbox = f.mailbox("202").await;
    f.plugin.provision(mailbox.clone());
    f.provider.accepted().await;
    f.provider.release.add_permits(1);
    f.settle().await;
    assert_eq!(
        f.plugin
            .store
            .mailbox("202", &mailbox.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        "ready"
    );
    assert_eq!(
        f.count().await,
        1,
        "foreign_customer_completion_cleared_unrelated_effect"
    );
    for _ in 0..2 {
        f.plugin
            .store
            .mailbox_ready(&mailbox.id, "account")
            .await
            .unwrap();
        f.plugin.store.engine_state("202", "ready").await.unwrap();
    }
    unrelated.finish().await.unwrap();
    assert_eq!(f.count().await, 0);
    let calls_before_resume = f.provider.calls.load(Ordering::SeqCst);
    f.plugin.resume().await.unwrap();
    tokio::task::yield_now().await;
    f.settle().await;
    assert!(f.provider.calls.load(Ordering::SeqCst) > calls_before_resume);
    assert_eq!(
        f.plugin
            .store
            .find_engine("202")
            .await
            .unwrap()
            .unwrap()
            .state,
        "ready"
    );
    assert_eq!(f.count().await, 0);
    f.plugin.store.engine_enabled("202", false).await.unwrap();
    f.plugin.reconcile("202".into());
    tokio::task::yield_now().await;
    f.settle().await;
    assert_eq!(
        f.plugin
            .store
            .find_engine("202")
            .await
            .unwrap()
            .unwrap()
            .state,
        "suspended"
    );
    assert_eq!(f.count().await, 0);
    let calls = f.provider.calls.load(Ordering::SeqCst);
    f.plugin.provision(mailbox); // Disabled incoming is proved before contacting CE.
    tokio::task::yield_now().await;
    f.settle().await;
    f.plugin.reconcile("999".into()); // No customer engine, no request.
    tokio::task::yield_now().await;
    f.settle().await;
    assert_eq!(f.provider.calls.load(Ordering::SeqCst), calls);
    assert_eq!(f.count().await, 0);
}

#[tokio::test]
#[ignore = "owned TLS PostgreSQL; plugins/mail/tests/fixtures/mailbox-recovery.py"]
async fn prevalidation_failure_never_submits_a_provider_request() {
    let f = Fixture::new(Outcome::Success).await;
    let mailbox = f.mailbox("101").await;
    // Local engine secret cannot be opened; the provider has never been contacted.
    f.db.execute(
        "UPDATE customer_engines SET admin_sealed=$1 WHERE customer_id='101'",
        &[&vec![0_u8]],
    )
    .await
    .unwrap();
    f.plugin.provision(mailbox.clone());
    tokio::task::yield_now().await;
    f.settle().await;
    assert_eq!(
        f.plugin
            .store
            .mailbox("101", &mailbox.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        "failed"
    );
    assert_eq!(f.provider.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.count().await, 0);
}

#[tokio::test]
#[ignore = "runner kills the process after the real CE request is accepted, then restarts owned PG"]
async fn cancelled_mailbox_process_prepare() {
    let f = Fixture::new(Outcome::Success).await;
    let mailbox = f.mailbox("101").await;
    f.plugin.provision(mailbox);
    f.provider.accepted().await;
    assert_eq!(f.count().await, 1);
    std::fs::write(
        std::env::var("WEBDOCK_MAIL_FIXTURE_ACCEPTED").unwrap(),
        b"accepted",
    )
    .unwrap();
    std::future::pending::<()>().await;
}

#[tokio::test]
#[ignore = "runner-owned restart after cancelled_mailbox_process_prepare"]
async fn cancelled_mailbox_process_verify() {
    let f = Fixture::new(Outcome::Success).await;
    assert_eq!(f.count().await, 1);
    f.provider.release.add_permits(4);
    f.plugin.resume().await.unwrap();
    tokio::task::yield_now().await;
    f.settle().await;
    assert_eq!(
        f.provider.calls.load(Ordering::SeqCst),
        0,
        "restart_replayed_unresolved_provider_effect"
    );
    f.closed().await;
}

#[tokio::test]
#[ignore = "owned TLS PostgreSQL; plugins/mail/tests/fixtures/mailbox-recovery.py"]
async fn reconciliation_success_without_metadata_persistence_retains_custody() {
    let f = Fixture::new(Outcome::Success).await;
    f.plugin.engine_password("101").await.unwrap();
    let refusal = if std::env::var("WEBDOCK_MAIL_FIXTURE_OUTCOME").unwrap() == "skip" {
        "RETURN NULL;"
    } else {
        "RAISE EXCEPTION 'fixture_metadata_failure';"
    };
    f.db.batch_execute(&format!("CREATE FUNCTION fixture_reject_state() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.state='ready' THEN {refusal} END IF; RETURN NEW; END $$; CREATE TRIGGER fixture_reject_state BEFORE UPDATE ON customer_engines FOR EACH ROW EXECUTE FUNCTION fixture_reject_state()")).await.unwrap();
    f.plugin.reconcile("101".into());
    tokio::task::yield_now().await;
    f.settle().await;
    assert!(f.provider.calls.load(Ordering::SeqCst) > 0);
    assert_eq!(
        f.plugin
            .store
            .find_engine("101")
            .await
            .unwrap()
            .unwrap()
            .state,
        "provisioning"
    );
    f.closed().await;
}

#[tokio::test]
#[ignore = "owned TLS PostgreSQL; plugins/mail/tests/fixtures/mailbox-recovery.py"]
async fn disconnected_metadata_writer_cannot_release_accepted_effect() {
    let f = Fixture::new(Outcome::Success).await;
    let mailbox = f.mailbox("101").await;
    f.plugin.provision(mailbox);
    f.provider.accepted().await;
    let pid: i32 = f
        .plugin
        .store
        .0
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    assert!(
        f.db.query_one("SELECT pg_terminate_backend($1)", &[&pid])
            .await
            .unwrap()
            .get::<_, bool>(0)
    );
    f.provider.release.add_permits(1);
    f.settle().await;
    assert_eq!(f.provider.accounts.load(Ordering::SeqCst), 1);
    f.closed().await;
}

#[tokio::test]
#[ignore = "owned TLS PostgreSQL; plugins/mail/tests/fixtures/mailbox-recovery.py"]
async fn missing_mailbox_outcome_row_retains_custody() {
    let f = Fixture::new(Outcome::Success).await;
    let mailbox = f.mailbox("101").await;
    f.plugin.provision(mailbox.clone());
    f.provider.accepted().await;
    assert_eq!(
        f.db.execute("DELETE FROM mailboxes WHERE id=$1", &[&mailbox.id])
            .await
            .unwrap(),
        1
    );
    f.provider.release.add_permits(1);
    f.settle().await;
    assert!(
        f.plugin
            .store
            .mailbox("101", &mailbox.id)
            .await
            .unwrap()
            .is_none()
    );
    f.closed().await;
}

#[tokio::test]
#[ignore = "owned TLS PostgreSQL; plugins/mail/tests/fixtures/mailbox-recovery.py"]
async fn missing_engine_outcome_row_retains_custody() {
    let f = Fixture::new(Outcome::PauseApply).await;
    f.plugin.engine_password("101").await.unwrap();
    f.plugin.reconcile("101".into());
    f.provider.accepted().await;
    assert_eq!(
        f.db.execute("DELETE FROM customer_engines WHERE customer_id='101'", &[])
            .await
            .unwrap(),
        1
    );
    f.provider.release.add_permits(1);
    f.settle().await;
    assert!(f.plugin.store.find_engine("101").await.unwrap().is_none());
    f.closed().await;
}
