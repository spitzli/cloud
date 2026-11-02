//! Controlled provider faults exercise the persisted engine, not an imitation ledger.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
mod support;
use webdock_object_storage_plugin::{engine::Engine, model::*, provider::Provider, store::Store};
use webdock_plugin_kit::seal::Sealer;

#[derive(Default)]
struct Remote {
    buckets: BTreeMap<String, Binding>,
    keys: Vec<String>,
    sequence: u32,
    fail_key: bool,
    fail_before_key: bool,
    fail_observe_after_key: bool,
    fail_observe: bool,
    collision_key: bool,
    invalidate_scan: bool,
    unknown_scan: bool,
    lifecycle_wait: Option<Arc<tokio::sync::Notify>>,
    lifecycle_started: Option<Arc<tokio::sync::Notify>>,
    revocations: u32,
    observation_wait: Option<Arc<tokio::sync::Notify>>,
    observation_started: Option<Arc<tokio::sync::Notify>>,
    head_wait: Option<Arc<tokio::sync::Notify>>,
    head_started: Option<Arc<tokio::sync::Notify>>,
    heads: u32,
    lifecycle: Option<Lifecycle>,
    objects: u64,
    paginated: bool,
}
#[derive(Clone, Default)]
struct Controlled(Arc<Mutex<Remote>>);
#[async_trait::async_trait]
impl Provider for Controlled {
    async fn head_object(
        &self,
        _: &Binding,
        key: &str,
    ) -> ProviderResult<webdock_object_storage_plugin::data::ObjectMetadata> {
        let waiting = {
            let mut r = self.0.lock().unwrap();
            r.heads += 1;
            r.head_wait.clone().zip(r.head_started.clone())
        };
        if let Some((wait, started)) = waiting {
            started.notify_one();
            wait.notified().await;
        }
        Ok(webdock_object_storage_plugin::data::ObjectMetadata {
            key: key.into(),
            exact_bytes: 3,
            version_id: "null".into(),
            if_match: "\"abc123\"".into(),
            filename: "proof.txt".into(),
            content_type: "text/plain".into(),
            expected_sha256: None,
        })
    }
    fn backend(&self) -> &str {
        "local-test"
    }
    fn endpoint(&self) -> &str {
        "http://127.0.0.1:9000"
    }
    async fn presign(
        &self,
        _: &Binding,
        _: webdock_plugin_protocol::storage::StorageDirection,
        _: &webdock_object_storage_plugin::data::DataRequest,
    ) -> ProviderResult<webdock_object_storage_plugin::data::PresignedObject> {
        Ok(webdock_object_storage_plugin::data::PresignedObject {
            url: "http://127.0.0.1:9000/private?signature=private-plan".into(),
            headers: BTreeMap::new(),
        })
    }
    async fn provision(&self, b: &Binding) -> ProviderResult<()> {
        let mut r = self.0.lock().unwrap();
        if r.buckets.contains_key(&b.bucket) {
            return Err(ProviderError::Collision);
        }
        r.buckets.insert(b.bucket.clone(), b.clone());
        Ok(())
    }
    async fn observe(&self, b: &Binding) -> ProviderResult<Observation> {
        let waiting = {
            let r = self.0.lock().unwrap();
            if r.sequence > 0 {
                r.observation_wait
                    .clone()
                    .zip(r.observation_started.clone())
            } else {
                None
            }
        };
        if let Some((wait, started)) = waiting {
            started.notify_one();
            wait.notified().await;
        }
        let r = self.0.lock().unwrap();
        if r.fail_observe {
            return Err(ProviderError::Unknown);
        }
        let owned = r.buckets.get(&b.bucket).is_some_and(|v| v == b);
        Ok(Observation {
            exists: r.buckets.contains_key(&b.bucket),
            owned,
            versioning: false,
            lifecycle: r.lifecycle.clone(),
            observed_at: now(),
        })
    }
    async fn keys(&self, _: &Binding) -> ProviderResult<Vec<String>> {
        Ok(self.0.lock().unwrap().keys.clone())
    }
    async fn create_key(&self, _: &Binding) -> ProviderResult<SecretKey> {
        let mut r = self.0.lock().unwrap();
        r.sequence += 1;
        if r.fail_before_key {
            return Err(ProviderError::Unknown);
        }
        let id = if r.collision_key {
            "key1".into()
        } else {
            format!("key{}", r.sequence)
        };
        r.keys.push(id.clone());
        if r.fail_key {
            return Err(ProviderError::Unknown);
        }
        if r.fail_observe_after_key {
            r.fail_observe = true;
        }
        Ok(SecretKey {
            access_key: id,
            secret_key: "private-app-secret".into(),
        })
    }
    async fn revoke_key(&self, _: &Binding, key: &str) -> ProviderResult<()> {
        let mut remote = self.0.lock().unwrap();
        remote.revocations += 1;
        remote.keys.retain(|k| k != key);
        Ok(())
    }
    async fn set_versioning(&self, _: &Binding, _: bool) -> ProviderResult<()> {
        Ok(())
    }
    async fn set_lifecycle(&self, _: &Binding, p: Option<&Lifecycle>) -> ProviderResult<()> {
        let (wait, started) = {
            let r = self.0.lock().unwrap();
            (r.lifecycle_wait.clone(), r.lifecycle_started.clone())
        };
        if let Some(started) = started {
            started.notify_one();
        }
        if let Some(wait) = wait {
            wait.notified().await;
        }
        self.0.lock().unwrap().lifecycle = p.cloned();
        Ok(())
    }
    async fn inventory(
        &self,
        _: &Binding,
        cursor: Option<&Cursor>,
    ) -> ProviderResult<InventoryPage> {
        let r = self.0.lock().unwrap();
        if r.invalidate_scan && cursor.is_some() {
            return Err(ProviderError::ScanInvalidated);
        }
        if r.unknown_scan {
            return Err(ProviderError::Unknown);
        }
        Ok(InventoryPage {
            current_bytes: r.objects,
            next: if r.paginated && cursor.is_none() {
                Some(Cursor::Uploads {
                    key: None,
                    upload: None,
                })
            } else {
                None
            },
            ..Default::default()
        })
    }
    async fn purge(&self, _: &Binding) -> ProviderResult<bool> {
        self.0.lock().unwrap().objects = 0;
        Ok(true)
    }
    async fn delete_empty(&self, b: &Binding) -> ProviderResult<()> {
        let mut r = self.0.lock().unwrap();
        if r.objects > 0 {
            return Err(ProviderError::Unknown);
        }
        r.buckets.remove(&b.bucket);
        Ok(())
    }
}
#[tokio::test]
#[ignore = "requires fixtures/postgres.py fresh owned PostgreSQL"]
async fn object_observation_is_scoped_private_metadata_and_rechecks_active_epoch_after_head() {
    let f = Fixture::new().await;
    let id = f.create().await;
    support::released(f.engine.store()).await;
    let input = json!({"resource_id":id,"key":"reports/proof.txt"});
    let observed = f.call("objects.observe", input.clone()).await.unwrap();
    assert_eq!(observed["key"], "reports/proof.txt");
    assert_eq!(observed["exact_bytes"], 3);
    for field in [
        "url",
        "headers",
        "secret_key",
        "access_key",
        "endpoint",
        "bucket",
        "expected_sha256",
    ] {
        assert!(observed.get(field).is_none());
    }
    assert!(
        f.engine
            .execute("foreign", "project-a", "objects.observe", input.clone())
            .await
            .is_err()
    );
    assert!(
        f.engine
            .execute("customer-a", "foreign", "objects.observe", input.clone())
            .await
            .is_err()
    );
    assert_eq!(f.provider.0.lock().unwrap().heads, 1);
    let started = Arc::new(tokio::sync::Notify::new());
    let wait = Arc::new(tokio::sync::Notify::new());
    {
        let mut remote = f.provider.0.lock().unwrap();
        remote.head_wait = Some(wait.clone());
        remote.head_started = Some(started.clone());
    }
    let engine = Engine::new(
        f.db.store().await,
        Sealer::new(&"42".repeat(32)).unwrap(),
        Arc::new(f.provider.clone()),
    );
    let task = tokio::spawn(async move {
        engine
            .execute("customer-a", "project-a", "objects.observe", input)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    let fence = f.engine.store().lock().await.unwrap();
    let mut resource = fence.get(&id.parse().unwrap()).await.unwrap().unwrap();
    resource.pending = Some("buckets.deactivate".into());
    resource.revision += 1;
    fence.save(&resource).await.unwrap();
    drop(fence);
    wait.notify_one();
    assert!(task.await.unwrap().is_err());
}
struct Fixture {
    db: support::Database,
    engine: Engine,
    provider: Controlled,
}
impl Fixture {
    async fn new() -> Self {
        let db = support::Database::new().await;
        let provider = Controlled::default();
        let engine = Engine::new(
            db.store().await,
            Sealer::new(&"42".repeat(32)).unwrap(),
            Arc::new(provider.clone()),
        );
        Self {
            db,
            engine,
            provider,
        }
    }
    async fn call(&self, action: &str, input: Value) -> Result<Value, String> {
        self.engine
            .execute("customer-a", "project-a", action, input)
            .await
            .map_err(|e| e.to_string())
    }
    async fn create(&self) -> String {
        self.call("buckets.create",json!({"request_id":uuid::Uuid::new_v4(),"name":"App files","environment":"production"})).await.unwrap()["resource"]["id"].as_str().unwrap().into()
    }
    async fn change(&self, id: &str, action: &str, extra: Value) -> Result<Value, String> {
        let v = self.call("buckets.view", json!({"resource_id":id})).await?;
        let mut input =
            json!({"resource_id":id,"request_id":uuid::Uuid::new_v4(),"revision":v["revision"]});
        input
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        self.call(action, input).await
    }
}
#[tokio::test]
#[ignore = "requires fixtures/postgres.py fresh owned PostgreSQL"]
async fn binding_ownership_and_replay_survive_restart() {
    let f = Fixture::new().await;
    let request = json!({"request_id":uuid::Uuid::new_v4(),"name":"Files","environment":"staging"});
    let first = f.call("buckets.create", request.clone()).await.unwrap();
    let id = first["resource"]["id"].as_str().unwrap();
    let restarted = Engine::new(
        f.db.store().await,
        Sealer::new(&"42".repeat(32)).unwrap(),
        Arc::new(f.provider.clone()),
    );
    assert_eq!(
        restarted
            .execute("customer-a", "project-a", "buckets.create", request.clone())
            .await
            .unwrap(),
        first
    );
    assert!(
        restarted
            .execute(
                "customer-b",
                "project-a",
                "buckets.view",
                json!({"resource_id":id})
            )
            .await
            .is_err()
    );
    assert!(
        restarted
            .execute(
                "customer-a",
                "project-b",
                "buckets.view",
                json!({"resource_id":id})
            )
            .await
            .is_err()
    );
    let mut changed = request;
    changed["environment"] = json!("production");
    assert!(f.call("buckets.create", changed).await.is_err());
    assert!(
        f.change(id, "buckets.reactivate", json!({"environment":"elsewhere"}))
            .await
            .is_err()
    );
}
#[tokio::test]
#[ignore = "requires fixtures/postgres.py fresh owned PostgreSQL"]
async fn rotation_requires_overlap_then_explicit_revoke_and_has_two_key_ceiling() {
    let f = Fixture::new().await;
    let id = f.create().await;
    let key = f
        .change(&id, "credentials.create", json!({}))
        .await
        .unwrap()["access_key"]
        .as_str()
        .unwrap()
        .to_owned();
    let second = f
        .change(&id, "credentials.rotate", json!({}))
        .await
        .unwrap()["access_key"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(key, second);
    assert!(
        f.change(&id, "credentials.rotate", json!({}))
            .await
            .is_err()
    );
    let restarted = Engine::new(
        f.db.store().await,
        Sealer::new(&"42".repeat(32)).unwrap(),
        Arc::new(f.provider.clone()),
    );
    let reveal = restarted
        .execute(
            "customer-a",
            "project-a",
            "credentials.reveal",
            json!({"resource_id":id,"access_key":second}),
        )
        .await
        .unwrap();
    assert_eq!(reveal["secret_key"], "private-app-secret");
    f.change(&id, "credentials.revoke", json!({"access_key":key}))
        .await
        .unwrap();
    assert!(
        f.call(
            "credentials.reveal",
            json!({"resource_id":id,"access_key":key})
        )
        .await
        .is_err()
    );
    let rows =
        f.db.client()
            .await
            .query(
                "SELECT body FROM resources UNION ALL SELECT COALESCE(result,'') FROM operations",
                &[],
            )
            .await
            .unwrap();
    for row in rows {
        assert!(!row.get::<_, String>(0).contains("private-app-secret"));
    }
}
#[tokio::test]
#[ignore = "requires fixtures/postgres.py fresh owned PostgreSQL"]
async fn ambiguous_key_creation_is_never_retried_and_explicit_reconciliation_revokes_unknown_key() {
    let f = Fixture::new().await;
    let id = f.create().await;
    f.provider.0.lock().unwrap().fail_key = true;
    let revision = f
        .call("buckets.view", json!({"resource_id":id}))
        .await
        .unwrap()["revision"]
        .clone();
    let request = json!({"resource_id":id,"request_id":uuid::Uuid::new_v4(),"revision":revision});
    assert!(f.call("credentials.create", request.clone()).await.is_err());
    assert!(f.call("credentials.create", request).await.is_err());
    assert_eq!(f.provider.0.lock().unwrap().sequence, 1);
    assert_eq!(
        f.call("buckets.view", json!({"resource_id":id}))
            .await
            .unwrap()["state"],
        "needs-reconciliation"
    );
    f.change(&id, "buckets.reconcile", json!({})).await.unwrap();
    assert!(f.provider.0.lock().unwrap().keys.is_empty());
    assert_eq!(
        f.call("buckets.view", json!({"resource_id":id}))
            .await
            .unwrap()["state"],
        "inactive"
    );
}
#[tokio::test]
#[ignore = "requires fixtures/postgres.py fresh owned PostgreSQL"]
async fn deactivation_retains_bytes_suspends_lifecycle_and_never_restores_revoked_keys() {
    let f = Fixture::new().await;
    let id = f.create().await;
    f.change(&id, "credentials.create", json!({}))
        .await
        .unwrap();
    f.change(&id,"lifecycle.set",json!({"policy":{"expire_current_days":30,"expire_noncurrent_days":40,"abort_multipart_days":7}})).await.unwrap();
    f.provider.0.lock().unwrap().objects = 123;
    f.change(&id, "buckets.deactivate", json!({}))
        .await
        .unwrap();
    {
        let r = f.provider.0.lock().unwrap();
        assert_eq!(r.objects, 123);
        assert!(r.keys.is_empty());
        assert!(r.lifecycle.is_none());
    }
    assert!(
        f.change(&id, "credentials.create", json!({}))
            .await
            .is_err()
    );
    f.change(&id, "buckets.reactivate", json!({}))
        .await
        .unwrap();
    assert!(f.provider.0.lock().unwrap().keys.is_empty());
    assert!(f.provider.0.lock().unwrap().lifecycle.is_none());
}
#[tokio::test]
#[ignore = "requires fixtures/postgres.py fresh owned PostgreSQL"]
async fn purge_is_separate_and_delete_requires_verified_empty_inactive_bucket() {
    let f = Fixture::new().await;
    let id = f.create().await;
    f.provider.0.lock().unwrap().objects = 12;
    assert!(
        f.change(&id, "buckets.delete", json!({"confirm":id}))
            .await
            .is_err()
    );
    f.change(&id, "buckets.deactivate", json!({}))
        .await
        .unwrap();
    assert!(
        f.change(&id, "buckets.delete", json!({"confirm":id}))
            .await
            .is_err()
    );
    f.change(&id, "buckets.purge", json!({"confirm":id}))
        .await
        .unwrap();
    f.change(&id, "buckets.delete", json!({"confirm":id}))
        .await
        .unwrap();
    assert!(f.provider.0.lock().unwrap().buckets.is_empty());
}
#[tokio::test]
#[ignore = "requires fixtures/postgres.py fresh owned PostgreSQL"]
async fn independent_store_handles_are_fenced() {
    let f = Fixture::new().await;
    let other = f.db.store().await;
    let guard = f.engine.store().lock().await.unwrap();
    assert!(other.lock().await.is_err());
    drop(guard);
    support::released(&other).await;
    assert!(other.lock().await.is_ok());
}
#[tokio::test]
#[ignore = "requires fixtures/postgres.py fresh owned PostgreSQL"]
async fn stale_revisions_and_inactive_reveals_never_contact_key_creation() {
    let f = Fixture::new().await;
    let id = f.create().await;
    let request = json!({"resource_id":id,"request_id":uuid::Uuid::new_v4(),"revision":0});
    assert!(f.call("credentials.create", request).await.is_err());
    assert_eq!(f.provider.0.lock().unwrap().sequence, 0);
    let key = f
        .change(&id, "credentials.create", json!({}))
        .await
        .unwrap()["access_key"]
        .clone();
    f.change(&id, "buckets.deactivate", json!({}))
        .await
        .unwrap();
    assert!(
        f.call(
            "credentials.reveal",
            json!({"resource_id":id,"access_key":key})
        )
        .await
        .is_err()
    );
}
#[tokio::test]
#[ignore = "requires fixtures/postgres.py fresh owned PostgreSQL"]
async fn encrypted_credentials_cannot_be_transplanted_between_resource_identities() {
    let f = Fixture::new().await;
    let a = f.create().await;
    let b = f.create().await;
    let key = f.change(&a, "credentials.create", json!({})).await.unwrap()["access_key"]
        .as_str()
        .unwrap()
        .to_owned();
    let fence = f.engine.store().lock().await.unwrap();
    let mut target = fence.get(&b.parse().unwrap()).await.unwrap().unwrap();
    target.credentials = fence
        .get(&a.parse().unwrap())
        .await
        .unwrap()
        .unwrap()
        .credentials;
    fence.save(&target).await.unwrap();
    drop(fence);
    support::released(f.engine.store()).await;
    assert!(
        f.call(
            "credentials.reveal",
            json!({"resource_id":b,"access_key":key})
        )
        .await
        .is_err()
    );
}

#[tokio::test]
#[ignore = "requires fixtures/postgres.py fresh owned PostgreSQL"]
async fn final_delete_walks_both_empty_inventory_categories() {
    let f = Fixture::new().await;
    let id = f.create().await;
    f.provider.0.lock().unwrap().paginated = true;
    f.change(&id, "buckets.deactivate", json!({}))
        .await
        .unwrap();
    f.change(&id, "buckets.delete", json!({"confirm":id}))
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires fixtures/postgres.py fresh owned PostgreSQL"]
async fn absent_key_after_ambiguous_creation_does_not_prove_the_remote_operation_finished() {
    let f = Fixture::new().await;
    let id = f.create().await;
    f.provider.0.lock().unwrap().fail_before_key = true;
    assert!(
        f.change(&id, "credentials.create", json!({}))
            .await
            .is_err()
    );
    assert!(f.change(&id, "buckets.reconcile", json!({})).await.is_err());
    assert_eq!(
        f.call("buckets.view", json!({"resource_id":id}))
            .await
            .unwrap()["state"],
        "needs-reconciliation"
    );
}

#[tokio::test]
#[ignore = "spawned only by cross_process_fence"]
async fn fence_child() {
    let name = std::env::var("OBJECT_STORAGE_FENCE_TEST_DATABASE").expect("child database");
    let store = Store::connect(&support::database_url(&name), None)
        .await
        .unwrap();
    let held = std::env::var("OBJECT_STORAGE_FENCE_HELD").unwrap() == "true";
    assert_eq!(
        store.lock().await.is_err(),
        held,
        "cross-process session fence"
    );
}
#[tokio::test]
#[ignore = "requires fixtures/postgres.py fresh owned PostgreSQL"]
async fn cross_process_fence() {
    let f = Fixture::new().await;
    let guard = f.engine.store().lock().await.unwrap();
    let child = |held: &str| {
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "fence_child", "--ignored"])
            .env("OBJECT_STORAGE_FENCE_TEST_DATABASE", &f.db.name)
            .env("OBJECT_STORAGE_FENCE_HELD", held);
        command
    };
    assert!(child("true").status().await.unwrap().success());
    // The first child exited without unlocking the parent's session.
    drop(guard);
    support::released(f.engine.store()).await;
    assert!(child("false").status().await.unwrap().success());
}
#[tokio::test]
#[ignore = "requires fixtures/postgres.py fresh owned PostgreSQL"]
async fn cancelled_provider_request_leaves_durable_reconciliation_and_no_replay() {
    #[derive(Clone)]
    struct Waiting(Controlled, Arc<tokio::sync::Notify>);
    #[async_trait::async_trait]
    impl Provider for Waiting {
        fn backend(&self) -> &str {
            self.0.backend()
        }
        fn endpoint(&self) -> &str {
            self.0.endpoint()
        }
        async fn provision(&self, b: &Binding) -> ProviderResult<()> {
            self.0.provision(b).await
        }
        async fn observe(&self, b: &Binding) -> ProviderResult<Observation> {
            self.0.observe(b).await
        }
        async fn keys(&self, b: &Binding) -> ProviderResult<Vec<String>> {
            self.0.keys(b).await
        }
        async fn create_key(&self, b: &Binding) -> ProviderResult<SecretKey> {
            let _key = self.0.create_key(b).await?;
            self.1.notify_one();
            std::future::pending().await
        }
        async fn revoke_key(&self, b: &Binding, key: &str) -> ProviderResult<()> {
            self.0.revoke_key(b, key).await
        }
        async fn set_versioning(&self, b: &Binding, enabled: bool) -> ProviderResult<()> {
            self.0.set_versioning(b, enabled).await
        }
        async fn set_lifecycle(&self, b: &Binding, p: Option<&Lifecycle>) -> ProviderResult<()> {
            self.0.set_lifecycle(b, p).await
        }
        async fn inventory(
            &self,
            b: &Binding,
            c: Option<&Cursor>,
        ) -> ProviderResult<InventoryPage> {
            self.0.inventory(b, c).await
        }
        async fn purge(&self, b: &Binding) -> ProviderResult<bool> {
            self.0.purge(b).await
        }
        async fn delete_empty(&self, b: &Binding) -> ProviderResult<()> {
            self.0.delete_empty(b).await
        }
    }
    let f = Fixture::new().await;
    let id = f.create().await;
    let view = f
        .call("buckets.view", json!({"resource_id":id}))
        .await
        .unwrap();
    let input =
        json!({"resource_id":id,"request_id":uuid::Uuid::new_v4(),"revision":view["revision"]});
    let started = Arc::new(tokio::sync::Notify::new());
    let e = Engine::new(
        f.db.store().await,
        Sealer::new(&"42".repeat(32)).unwrap(),
        Arc::new(Waiting(f.provider.clone(), started.clone())),
    );
    let request = input.clone();
    let task = tokio::spawn(async move {
        e.execute("customer-a", "project-a", "credentials.create", request)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    // A separate PG session can see the committed intent while external work is suspended.
    let row = f.db.client().await.query_one("SELECT body, result FROM resources JOIN operations ON resources.id=operations.resource WHERE operations.id=$1", &[&input["request_id"].as_str().unwrap().parse::<uuid::Uuid>().unwrap()]).await.unwrap();
    let pending: Value = serde_json::from_str(row.get::<_, &str>(0)).unwrap();
    assert_eq!(pending["pending"], "credentials.create");
    assert_eq!(pending["state"], "needs-reconciliation");
    assert!(row.get::<_, Option<String>>(1).is_none());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    support::released(f.engine.store()).await;
    assert!(f.call("credentials.create", input).await.is_err());
    assert_eq!(f.provider.0.lock().unwrap().sequence, 1);
    assert_eq!(
        f.call("buckets.view", json!({"resource_id":id}))
            .await
            .unwrap()["state"],
        "needs-reconciliation"
    );
}

#[tokio::test]
#[ignore = "requires fixtures/postgres.py fresh owned PostgreSQL"]
async fn newer_metadata_schema_fails_closed_without_modifying_it() {
    let f = Fixture::new().await;
    f.db.client()
        .await
        .execute("INSERT INTO schema_migrations VALUES (2)", &[])
        .await
        .unwrap();
    assert!(Store::connect(&f.db.url, None).await.is_err());
    assert_eq!(
        f.db.client()
            .await
            .query_one("SELECT max(version) FROM schema_migrations", &[])
            .await
            .unwrap()
            .get::<_, i32>(0),
        2
    );
}

#[tokio::test]
async fn metadata_transport_rejects_remote_plaintext_and_invalid_ca_without_logging_url() {
    assert!(matches!(
        Store::connect("postgresql://must-not-be-printed@metadata.invalid/db", None).await,
        Err("plaintext metadata requires loopback")
    ));
    assert!(matches!(
        Store::connect(
            "postgresql://must-not-be-printed@metadata.invalid/db",
            Some(b"invalid")
        )
        .await,
        Err("metadata migration unavailable")
    ));
}

// Two independently launched processes around a restart of ONLY the runner's owned container.
#[tokio::test]
#[ignore = "orchestrated in two phases by fixtures/postgres.py"]
async fn postgres_server_restart_preserves_intent_sealed_credentials_and_replay() {
    let path = std::path::PathBuf::from(
        std::env::var_os("WEBDOCK_OBJECT_STORAGE_POSTGRES_CONFIG").unwrap(),
    )
    .parent()
    .unwrap()
    .join("restart.json");
    if std::env::var("OBJECT_STORAGE_RESTART_PHASE").unwrap() == "write" {
        let f = Fixture::new().await;
        let request = json!({"request_id":uuid::Uuid::new_v4(),"name":"Restart proof","environment":"staging"});
        let created = f.call("buckets.create", request.clone()).await.unwrap();
        let id = created["resource"]["id"].as_str().unwrap();
        let key =
            f.change(id, "credentials.create", json!({})).await.unwrap()["access_key"].clone();
        let pending = uuid::Uuid::new_v4();
        let mut fence = f.engine.store().lock().await.unwrap();
        let mut resource = fence.get(&id.parse().unwrap()).await.unwrap().unwrap();
        resource.revision += 1;
        resource.state = "needs-reconciliation".into();
        resource.pending = Some("versioning.set".into());
        fence
            .begin(&pending, "restart-pending", &resource)
            .await
            .unwrap();
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        file.write_all(json!({"database":f.db.name,"request":request,"created":created,"key":key,"pending":pending}).to_string().as_bytes()).unwrap();
    } else {
        let proof: Value =
            serde_json::from_slice(&webdock_plugin_kit::read_file(&path, true).unwrap()).unwrap();
        let url = support::database_url(proof["database"].as_str().unwrap());
        let store = Store::connect(&url, None).await.unwrap();
        let fence = store.lock().await.unwrap();
        let r = fence
            .get(
                &proof["created"]["resource"]["id"]
                    .as_str()
                    .unwrap()
                    .parse()
                    .unwrap(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r.state, "needs-reconciliation");
        assert_eq!(r.pending.as_deref(), Some("versioning.set"));
        assert!(
            fence
                .replay(
                    &proof["pending"].as_str().unwrap().parse().unwrap(),
                    "restart-pending"
                )
                .await
                .is_err()
        );
        assert_eq!(r.credentials.len(), 1);
        let secret = &r.credentials[0];
        drop(fence);
        support::released(&store).await;
        let remote = Controlled::default();
        {
            let mut state = remote.0.lock().unwrap();
            state
                .buckets
                .insert(r.binding.bucket.clone(), r.binding.clone());
            state.keys.push(secret.access_key.clone());
        }
        let engine = Engine::new(
            store,
            Sealer::new(&"42".repeat(32)).unwrap(),
            Arc::new(remote),
        );
        assert_eq!(
            engine
                .execute(
                    "customer-a",
                    "project-a",
                    "buckets.create",
                    proof["request"].clone()
                )
                .await
                .unwrap(),
            proof["created"]
        );
        // Pending state correctly blocks reveal, even with retained ciphertext and a valid remote key.
        assert!(
            engine
                .execute(
                    "customer-a",
                    "project-a",
                    "credentials.reveal",
                    json!({"resource_id":r.binding.id,"access_key":proof["key"]})
                )
                .await
                .is_err()
        );
        // Test-only repair: no provider setting was attempted for this synthetic pending intent.
        let fence = engine.store().lock().await.unwrap();
        let mut restored = r;
        restored.pending = None;
        restored.state = "active".into();
        fence.save(&restored).await.unwrap();
        drop(fence);
        support::released(engine.store()).await;
        assert_eq!(
            engine
                .execute(
                    "customer-a",
                    "project-a",
                    "credentials.reveal",
                    json!({"resource_id":restored.binding.id,"access_key":proof["key"]})
                )
                .await
                .unwrap()["secret_key"],
            "private-app-secret"
        );
        std::fs::remove_file(path).unwrap();
    }
}

#[tokio::test]
#[ignore = "requires fixtures/postgres.py fresh owned PostgreSQL"]
async fn returned_key_survives_failed_observation_and_recovers_conservatively() {
    let f = Fixture::new().await;
    let id = f.create().await;
    f.provider.0.lock().unwrap().fail_observe_after_key = true;
    assert!(
        f.change(&id, "credentials.create", json!({}))
            .await
            .is_err()
    );
    f.provider.0.lock().unwrap().fail_observe = false;
    let fence = f.engine.store().lock().await.unwrap();
    let r = fence.get(&id.parse().unwrap()).await.unwrap().unwrap();
    assert_eq!(r.credentials.len(), 1);
    assert!(!r.credentials[0].sealed.is_empty());
    drop(fence);
    support::released(f.engine.store()).await;
    f.change(&id, "buckets.reconcile", json!({})).await.unwrap();
    assert!(f.provider.0.lock().unwrap().keys.is_empty());
    assert_eq!(f.provider.0.lock().unwrap().sequence, 1);
    assert_eq!(
        f.call("buckets.view", json!({"resource_id":id}))
            .await
            .unwrap()["state"],
        "inactive"
    );
}

#[tokio::test]
#[ignore = "requires fixtures/postgres.py fresh owned PostgreSQL"]
async fn returned_credential_collision_is_not_treated_as_known_success() {
    let f = Fixture::new().await;
    let id = f.create().await;
    f.change(&id, "credentials.create", json!({}))
        .await
        .unwrap();
    f.provider.0.lock().unwrap().collision_key = true;
    assert!(
        f.change(&id, "credentials.rotate", json!({}))
            .await
            .is_err()
    );
    assert!(f.change(&id, "buckets.reconcile", json!({})).await.is_err());
    let fence = f.engine.store().lock().await.unwrap();
    assert_eq!(
        fence
            .get(&id.parse().unwrap())
            .await
            .unwrap()
            .unwrap()
            .credentials
            .len(),
        1
    );
}

#[tokio::test]
#[ignore = "requires fixtures/postgres.py fresh owned PostgreSQL"]
async fn invalidated_inventory_resets_without_revoking_keys_or_deactivating() {
    let f = Fixture::new().await;
    let id = f.create().await;
    f.change(&id, "credentials.create", json!({}))
        .await
        .unwrap();
    f.provider.0.lock().unwrap().paginated = true;
    f.change(&id, "usage.inventory", json!({})).await.unwrap();
    f.provider.0.lock().unwrap().invalidate_scan = true;
    let reset = f.change(&id, "usage.inventory", json!({})).await.unwrap();
    assert_eq!(reset["scan_reset"], true);
    let v = f
        .call("buckets.view", json!({"resource_id":id}))
        .await
        .unwrap();
    assert_eq!(v["state"], "active");
    assert_eq!(v["credentials"].as_array().unwrap().len(), 1);
    assert!(v["inventory"].is_null());
    f.provider.0.lock().unwrap().invalidate_scan = false;
    f.change(&id, "usage.inventory", json!({})).await.unwrap();
}

#[tokio::test]
#[ignore = "requires fixtures/postgres.py fresh owned PostgreSQL"]
async fn terminated_fence_cancels_suspended_deactivation_and_never_overtakes() {
    let f = Arc::new(Fixture::new().await);
    let id = f.create().await;
    f.change(&id, "credentials.create", json!({}))
        .await
        .unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    {
        let mut r = f.provider.0.lock().unwrap();
        r.lifecycle_wait = Some(resume.clone());
        r.lifecycle_started = Some(started.clone());
    }
    let child = f.clone();
    let resource = id.clone();
    let mut task = tokio::spawn(async move {
        child
            .change(&resource, "buckets.deactivate", json!({}))
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    let admin = f.db.client().await;
    let pid: i32 = admin
        .query_one(
            "SELECT pid FROM pg_locks WHERE locktype='advisory' AND objid=704611025 AND database=(SELECT oid FROM pg_database WHERE datname=current_database()) AND granted",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(
        admin
            .query_one("SELECT pg_terminate_backend($1)", &[&pid])
            .await
            .unwrap()
            .get::<_, bool>(0)
    );
    // Simulate another process/runtime using the same durable metadata and provider namespace.
    {
        let mut r = f.provider.0.lock().unwrap();
        r.lifecycle_wait = None;
        r.lifecycle_started = None;
    }
    let other = Engine::new(
        f.db.store().await,
        Sealer::new(&"42".repeat(32)).unwrap(),
        Arc::new(f.provider.clone()),
    );
    let v = other
        .execute(
            "customer-a",
            "project-a",
            "buckets.view",
            json!({"resource_id":id}),
        )
        .await
        .unwrap();
    let change = |action: &'static str| {
        let input =
            json!({"resource_id":id,"request_id":uuid::Uuid::new_v4(),"revision":v["revision"]});
        let e = &other;
        async move { e.execute("customer-a", "project-a", action, input).await }
    };
    let child_status = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "fence_child_lost_execution", "--ignored"])
        .env("OBJECT_STORAGE_FENCE_TEST_DATABASE", &f.db.name)
        .env("OBJECT_STORAGE_FENCE_RESOURCE", &id)
        .env("OBJECT_STORAGE_FENCE_REVISION", v["revision"].to_string())
        .status()
        .await
        .unwrap();
    assert!(child_status.success());
    let recovery = change("buckets.reconcile").await;
    let reactivation = change("buckets.reactivate").await;
    let create = change("credentials.create").await;
    // The suspended provider future must be dropped without waiting for it to resume.
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(3), &mut task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    resume.notify_one();
    assert!(
        recovery.is_err(),
        "a lost execution fence must never be auto-reconciled"
    );
    assert!(reactivation.is_err());
    assert!(create.is_err());
    let r = f.provider.0.lock().unwrap();
    assert_eq!(
        r.revocations, 0,
        "old sequence must not make any later provider mutation"
    );
    assert_eq!(r.keys.len(), 1);
}

#[tokio::test]
#[ignore = "requires fixtures/postgres.py fresh owned PostgreSQL"]
async fn failed_readonly_scan_can_restart_without_resource_reconciliation() {
    let f = Fixture::new().await;
    let id = f.create().await;
    f.provider.0.lock().unwrap().unknown_scan = true;
    assert!(f.change(&id, "usage.inventory", json!({})).await.is_err());
    f.provider.0.lock().unwrap().unknown_scan = false;
    let v = f.change(&id, "usage.inventory", json!({})).await.unwrap();
    assert_eq!(v["state"], "active");
    assert!(v["pending"].is_null());
}

#[tokio::test]
#[ignore = "spawned only by terminated_fence regression"]
async fn fence_child_lost_execution() {
    let database = std::env::var("OBJECT_STORAGE_FENCE_TEST_DATABASE").unwrap();
    let id = std::env::var("OBJECT_STORAGE_FENCE_RESOURCE").unwrap();
    let revision: u64 = std::env::var("OBJECT_STORAGE_FENCE_REVISION")
        .unwrap()
        .parse()
        .unwrap();
    let engine = Engine::new(
        Store::connect(&support::database_url(&database), None)
            .await
            .unwrap(),
        Sealer::new(&"42".repeat(32)).unwrap(),
        Arc::new(Controlled::default()),
    );
    for action in [
        "buckets.reconcile",
        "buckets.reactivate",
        "credentials.create",
    ] {
        let result = engine
            .execute(
                "customer-a",
                "project-a",
                action,
                json!({"resource_id":id,"revision":revision,"request_id":uuid::Uuid::new_v4()}),
            )
            .await;
        assert!(matches!(
            result,
            Err(
                "Previous execution may still have external effects; private operator recovery required"
            )
        ));
    }
}

#[tokio::test]
#[ignore = "requires fixtures/postgres.py fresh owned PostgreSQL"]
async fn returned_key_evidence_commits_before_final_observation_without_reopening_resource() {
    let f = Arc::new(Fixture::new().await);
    let id = f.create().await;
    let wait = Arc::new(tokio::sync::Notify::new());
    let started = Arc::new(tokio::sync::Notify::new());
    {
        let mut r = f.provider.0.lock().unwrap();
        r.observation_wait = Some(wait);
        r.observation_started = Some(started.clone());
    }
    let child = f.clone();
    let resource = id.clone();
    let task = tokio::spawn(async move {
        child
            .change(&resource, "credentials.create", json!({}))
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    let row =
        f.db.client()
            .await
            .query_one(
                "SELECT body FROM resources WHERE id=$1",
                &[&id.parse::<uuid::Uuid>().unwrap()],
            )
            .await
            .unwrap();
    let stored: Value = serde_json::from_str(row.get::<_, &str>(0)).unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(stored["state"], "needs-reconciliation");
    assert_eq!(stored["executing"], true);
    assert_eq!(stored["returned_key"], "key1");
    assert_eq!(stored["credentials"].as_array().unwrap().len(), 1);
    assert!(
        !stored["credentials"][0]["sealed"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
#[ignore = "requires fixtures/postgres.py fresh owned PostgreSQL"]
async fn data_plans_are_fresh_private_and_never_journaled() {
    let f = Fixture::new().await;
    let id = f.create().await;
    let input = json!({"resource_id":id,"key":"reports/proof.txt","exact_bytes":3,"expected_sha256":"a".repeat(64),"filename":"proof.txt","content_type":"text/plain","expires_at":now()+120});
    let first = f.call("objects.upload", input.clone()).await;
    assert!(
        first.is_ok(),
        "active owned upload must produce a private plan"
    );
    let plan = first.unwrap();
    assert_eq!(plan["direction"], "upload");
    assert_eq!(plan["exactBytes"], 3);
    assert!(!plan["resourceEpoch"].as_str().unwrap().is_empty());
    let before: i64 =
        f.db.client()
            .await
            .query_one("SELECT count(*) FROM operations", &[])
            .await
            .unwrap()
            .get(0);
    assert!(f.call("objects.upload", input.clone()).await.is_ok());
    let after: i64 =
        f.db.client()
            .await
            .query_one("SELECT count(*) FROM operations", &[])
            .await
            .unwrap()
            .get(0);
    assert_eq!(before, after);
    for row in
        f.db.client()
            .await
            .query(
                "SELECT body FROM resources UNION ALL SELECT COALESCE(result,'') FROM operations",
                &[],
            )
            .await
            .unwrap()
    {
        assert!(!row.get::<_, String>(0).contains("private-plan"));
    }
    f.provider.0.lock().unwrap().fail_observe = true;
    assert!(f.call("objects.upload", input.clone()).await.is_err());
    f.provider.0.lock().unwrap().fail_observe = false;
    let buckets = f.provider.0.lock().unwrap().buckets.clone();
    f.provider.0.lock().unwrap().buckets.clear();
    assert!(
        f.call("objects.upload", input.clone()).await.is_err(),
        "unowned/missing provider bucket"
    );
    f.provider.0.lock().unwrap().buckets = buckets;
    for (field, value) in [
        ("method", json!("GET")),
        ("expected_sha256", json!("bad")),
        ("exact_bytes", json!(536870913_u64)),
        ("version_id", json!("version")),
        ("expires_at", json!(now() - 1)),
        ("key", json!("../escape")),
        ("content_type", json!("text/html")),
    ] {
        let mut bad = input.clone();
        bad[field] = value;
        assert!(
            f.call("objects.upload", bad).await.is_err(),
            "must reject {field}"
        );
    }
    assert!(
        f.engine
            .execute("other", "project-a", "objects.upload", input.clone())
            .await
            .is_err()
    );
    assert!(
        f.engine
            .execute("customer-a", "other", "objects.upload", input.clone())
            .await
            .is_err()
    );
    f.change(&id, "buckets.deactivate", json!({}))
        .await
        .unwrap();
    assert!(f.call("objects.upload", input.clone()).await.is_err());
    support::released(f.engine.store()).await;
    let client = f.db.client().await;
    let mut body: Value = serde_json::from_str(
        client
            .query_one(
                "SELECT body FROM resources WHERE id=$1",
                &[&id.parse::<uuid::Uuid>().unwrap()],
            )
            .await
            .unwrap()
            .get::<_, &str>(0),
    )
    .unwrap();
    let other = uuid::Uuid::new_v4();
    body["binding"]["id"] = json!(other);
    body["binding"]["bucket"] = json!(format!("wd-{}", other.simple()));
    body["binding"]["user"] = json!(format!("wd-{}", other.simple()));
    body["state"] = json!("active");
    client
        .execute(
            "UPDATE resources SET body=$2 WHERE id=$1",
            &[&id.parse::<uuid::Uuid>().unwrap(), &body.to_string()],
        )
        .await
        .unwrap();
    let rebound: Binding = serde_json::from_value(body["binding"].clone()).unwrap();
    f.provider
        .0
        .lock()
        .unwrap()
        .buckets
        .insert(rebound.bucket.clone(), rebound);
    // Typed producer itself must reject this mismatch, even if the provider owns the other body binding.
    assert!(
        f.engine
            .prepare_data(
                "customer-a",
                "project-a",
                webdock_plugin_protocol::storage::StorageDirection::Upload,
                input
            )
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires fixtures/postgres.py fresh owned PostgreSQL"]
async fn private_epoch_revalidation_denies_deactivation_pending_reactivation_and_wrong_scope_without_fence()
 {
    use webdock_plugin_protocol::storage::{StorageAuthorization, StorageDirection};
    let f = Fixture::new().await;
    let id = f
        .engine
        .execute(
            "1",
            "2",
            "buckets.create",
            json!({"request_id":uuid::Uuid::new_v4(),"name":"Data","environment":"test"}),
        )
        .await
        .unwrap()["resource"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    support::released(f.engine.store()).await;
    let plan = f.engine.execute("1","2","objects.upload",json!({"resource_id":id,"key":"proof.txt","exact_bytes":3,"expected_sha256":"a".repeat(64),"filename":"proof.txt","content_type":"text/plain","expires_at":now()+120})).await.unwrap();
    let a = StorageAuthorization {
        version: 1,
        transfer_id: uuid::Uuid::new_v4().to_string(),
        source_invocation: uuid::Uuid::new_v4().to_string(),
        plugin_id: "object-storage".into(),
        generation: 1,
        action: "objects.upload".into(),
        customer_id: "1".into(),
        project_id: "2".into(),
        resource_id: id.clone(),
        resource_epoch: plan["resourceEpoch"].as_str().unwrap().into(),
        direction: StorageDirection::Upload,
        exact_bytes: 3,
        expected_sha256: Some("a".repeat(64)),
        expires_at: now() as i64 + 120,
    };
    let fence = f.engine.store().lock().await.unwrap();
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(900),
            f.engine.authorize_data(&a)
        )
        .await
        .unwrap()
        .is_ok()
    );
    for field in [
        "customer", "project", "epoch", "action", "expires", "size", "checksum",
    ] {
        let mut bad = a.clone();
        match field {
            "customer" => bad.customer_id = "3".into(),
            "project" => bad.project_id = "3".into(),
            "epoch" => bad.resource_epoch = "other".into(),
            "action" => bad.action = "objects.download".into(),
            "expires" => bad.expires_at = now() as i64 - 1,
            "size" => bad.exact_bytes = 536870913,
            _ => bad.expected_sha256 = None,
        }
        assert!(f.engine.authorize_data(&bad).await.is_err(), "{field}");
    }
    let uuid = id.parse().unwrap();
    let mut r = fence.get(&uuid).await.unwrap().unwrap();
    r.pending = Some("buckets.deactivate".into());
    fence.save(&r).await.unwrap();
    assert!(f.engine.authorize_data(&a).await.is_err());
    r.pending = None;
    r.state = "inactive".into();
    r.revision += 1;
    fence.save(&r).await.unwrap();
    assert!(f.engine.authorize_data(&a).await.is_err());
    r.state = "active".into();
    r.revision += 1;
    fence.save(&r).await.unwrap();
    assert!(
        f.engine.authorize_data(&a).await.is_err(),
        "reactivation must not resurrect old tickets"
    );
    r.executing = true;
    fence.save(&r).await.unwrap();
    assert!(f.engine.authorize_data(&a).await.is_err());
}

#[tokio::test]
#[ignore = "requires fixtures/postgres.py fresh owned PostgreSQL"]
async fn reveal_is_the_only_secret_response_and_never_writes_operation_results() {
    let f = Fixture::new().await;
    let id = f.create().await;
    let created = f
        .change(&id, "credentials.create", json!({}))
        .await
        .unwrap();
    assert!(!created.to_string().contains("private-app-secret"));
    let key = created["access_key"].as_str().unwrap();
    support::released(f.engine.store()).await;
    let before: i64 =
        f.db.client()
            .await
            .query_one("SELECT count(*) FROM operations", &[])
            .await
            .unwrap()
            .get(0);
    let revealed = f
        .call(
            "credentials.reveal",
            json!({"resource_id":id,"access_key":key}),
        )
        .await
        .unwrap();
    for field in ["access_key", "secret_key", "endpoint", "bucket"] {
        assert!(revealed[field].is_string());
    }
    assert_eq!(revealed["secret_key"], "private-app-secret");
    support::released(f.engine.store()).await;
    let error = f
        .call(
            "credentials.reveal",
            json!({"resource_id":id,"access_key":"private-app-secret"}),
        )
        .await
        .unwrap_err();
    assert!(!error.contains("private-app-secret"));
    support::released(f.engine.store()).await;
    let view = f
        .call("buckets.view", json!({"resource_id":id}))
        .await
        .unwrap();
    assert!(!view.to_string().contains("private-app-secret"));
    let client = f.db.client().await;
    let after: i64 = client
        .query_one("SELECT count(*) FROM operations", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(before, after);
    for row in client
        .query(
            "SELECT body FROM resources UNION ALL SELECT COALESCE(result,'') FROM operations",
            &[],
        )
        .await
        .unwrap()
    {
        assert!(!row.get::<_, String>(0).contains("private-app-secret"));
    }
}

#[tokio::test]
#[ignore = "requires fixtures/postgres.py fresh owned PostgreSQL"]
async fn actual_deactivation_revokes_issued_plan_while_provider_is_suspended() {
    use webdock_plugin_protocol::storage::{StorageAuthorization, StorageDirection};
    let f = Fixture::new().await;
    let created = f
        .engine
        .execute(
            "1",
            "2",
            "buckets.create",
            json!({"request_id":uuid::Uuid::new_v4(),"name":"Data","environment":"test"}),
        )
        .await
        .unwrap();
    let id = created["resource"]["id"].as_str().unwrap();
    support::released(f.engine.store()).await;
    let plan=f.engine.execute("1","2","objects.upload",json!({"resource_id":id,"key":"live.txt","exact_bytes":3,"expected_sha256":"a".repeat(64),"filename":"live.txt","content_type":"text/plain","expires_at":now()+120})).await.unwrap();
    let authorization = StorageAuthorization {
        version: 1,
        transfer_id: uuid::Uuid::new_v4().to_string(),
        source_invocation: uuid::Uuid::new_v4().to_string(),
        plugin_id: "object-storage".into(),
        generation: 1,
        action: "objects.upload".into(),
        customer_id: "1".into(),
        project_id: "2".into(),
        resource_id: id.into(),
        resource_epoch: plan["resourceEpoch"].as_str().unwrap().into(),
        direction: StorageDirection::Upload,
        exact_bytes: 3,
        expected_sha256: Some("a".repeat(64)),
        expires_at: now() as i64 + 120,
    };
    assert!(f.engine.authorize_data(&authorization).await.is_ok());
    let wait = Arc::new(tokio::sync::Notify::new());
    let started = Arc::new(tokio::sync::Notify::new());
    {
        let mut remote = f.provider.0.lock().unwrap();
        remote.lifecycle_wait = Some(wait.clone());
        remote.lifecycle_started = Some(started.clone());
    }
    let deactivation = f.engine.execute(
        "1",
        "2",
        "buckets.deactivate",
        json!({"resource_id":id,"request_id":uuid::Uuid::new_v4(),"revision":created["revision"]}),
    );
    let during = async {
        started.notified().await;
        let denied = tokio::time::timeout(
            std::time::Duration::from_millis(900),
            f.engine.authorize_data(&authorization),
        )
        .await;
        wait.notify_one();
        assert!(
            denied.unwrap().is_err(),
            "committed pending deactivation must revoke an issued ticket while its global fence is held"
        );
    };
    let (deactivated, ()) = tokio::join!(deactivation, during);
    let deactivated = deactivated.unwrap();
    assert!(f.engine.authorize_data(&authorization).await.is_err());
    support::released(f.engine.store()).await;
    f.engine.execute("1","2","buckets.reactivate",json!({"resource_id":id,"request_id":uuid::Uuid::new_v4(),"revision":deactivated["revision"]})).await.unwrap();
    assert!(
        f.engine.authorize_data(&authorization).await.is_err(),
        "reactivation must not resurrect the issued plan"
    );
}
