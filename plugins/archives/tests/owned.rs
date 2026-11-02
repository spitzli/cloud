#![cfg(feature = "local-test")]
use serde::Deserialize;
use serde_json::json;
use std::{os::unix::fs::PermissionsExt, sync::Arc};
use webdock_archives::{
    Engine,
    backend::{Backend, BackendConfig},
    producer::DatabaseSource,
};
use webdock_plugin_host::storage::{StorageAuthority, StorageGateway};
use webdock_plugin_protocol::{PluginError, storage::StorageAuthorization};
#[derive(Deserialize)]
struct Fixture {
    database: String,
    source: String,
    backend: BackendConfig,
}
struct Live {
    engine: Arc<Engine>,
    a: StorageAuthorization,
}
#[async_trait::async_trait]
impl StorageAuthority for Live {
    async fn check(&self) -> Result<(), PluginError> {
        self.engine
            .authorize(&self.a)
            .await
            .map_err(|_| PluginError::denied())
    }
}
fn authorization(p: &webdock_plugin_protocol::storage::StoragePlan) -> StorageAuthorization {
    StorageAuthorization {
        version: 1,
        transfer_id: uuid::Uuid::new_v4().to_string(),
        source_invocation: uuid::Uuid::new_v4().to_string(),
        plugin_id: "archives".into(),
        generation: 1,
        action: "archives.download".into(),
        customer_id: "123".into(),
        project_id: "124".into(),
        resource_id: p.resource_id.clone(),
        resource_epoch: p.resource_epoch.clone(),
        direction: p.direction,
        exact_bytes: p.exact_bytes,
        expected_sha256: p.expected_sha256.clone(),
        expires_at: p.expires_at,
    }
}
#[tokio::test]
#[ignore = "owns exact MinIO resources and a disposable PostgreSQL source/metadata database"]
async fn durable_exports_diagnostics_and_binary_delivery_use_exact_owned_bytes() {
    let f: Fixture = serde_json::from_slice(
        &std::fs::read(std::env::var("WEBDOCK_ARCHIVES_FIXTURE").unwrap()).unwrap(),
    )
    .unwrap();
    let staging = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let binding = webdock_plugin_http::storage::StorageBinding {
        origin: f.backend.config.endpoint.clone(),
        addresses: vec!["127.0.0.1".parse().unwrap()],
        ca: None,
        path_prefix: format!("/{}/", f.backend.config.bucket),
    };
    let s3 = aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new(
                f.backend.config.region.clone(),
            ))
            .endpoint_url(&f.backend.config.endpoint)
            .credentials_provider(aws_sdk_s3::config::Credentials::new(
                f.backend.credentials.access_key_id.clone(),
                f.backend.credentials.secret_access_key.clone(),
                None,
                None,
                "owned-archive-test",
            ))
            .force_path_style(true)
            .build(),
    );
    let bucket = f.backend.config.bucket.clone();
    let backend = Arc::new(Backend::local_test(f.backend).await.unwrap());
    let source = DatabaseSource {
        customer: "123".into(),
        project: "124".into(),
        url: f.source,
    };
    assert!(
        Engine::connect(
            &f.database,
            None,
            backend.clone(),
            staging.path().into(),
            vec![source.clone()]
        )
        .await
        .is_err(),
        "normal Engine must reject legacy databaseSources even with fixture features compiled"
    );
    let engine = Arc::new(
        Engine::connect(
            &f.database,
            None,
            backend.clone(),
            staging.path().into(),
            vec![],
        )
        .await
        .unwrap(),
    );
    let id = uuid::Uuid::new_v4();
    assert!(engine.create("123", "124", json!({"request_id":uuid::Uuid::new_v4(),"kind":"postgres","retention_seconds":3600})).await.is_err(), "PG without source_id/managed binding is denied");
    let create = json!({"request_id":id,"kind":"diagnostics","retention_seconds":3600,"diagnostics":{"version":1,"state":"finished","builds":[{"id":"123","outcome":"succeeded","duration_ms":11,"output_bytes":42}],"checks":[]}});
    engine.create("123", "124", create.clone()).await.unwrap();
    assert!(engine.plan("123", "124", id).await.is_err());
    assert!(engine.produce("999", "124", id, "job-one").await.is_err());
    engine.produce("123", "124", id, "job-one").await.unwrap();
    let p = engine.plan("123", "124", id).await.unwrap();
    assert!(p.exact_bytes > 100);
    let before = p.expected_sha256.clone();
    let restarted = Arc::new(
        Engine::connect(
            &f.database,
            None,
            backend.clone(),
            staging.path().into(),
            vec![],
        )
        .await
        .unwrap(),
    );
    restarted.create("123", "124", create).await.unwrap();
    restarted
        .produce("123", "124", id, "job-one")
        .await
        .unwrap();
    assert_eq!(
        restarted
            .plan("123", "124", id)
            .await
            .unwrap()
            .expected_sha256,
        before
    );
    assert!(restarted.plan("123", "999", id).await.is_err());
    let gateway = StorageGateway::loopback_test(staging.path().into()).unwrap();

    let live = Arc::new(Live {
        engine: restarted.clone(),
        a: authorization(&p),
    });
    let stage = gateway
        .download(&p, gateway.target(&p, binding.clone()).unwrap(), live)
        .await
        .unwrap();
    let mut stream = stage.into_stream();
    let mut bytes = Vec::new();
    while let Some(c) = stream.next_chunk().await.unwrap() {
        bytes.extend(c);
    }
    assert_eq!(&bytes[..2], &[0x1f, 0x8b]);
    // The provider ETag is not a content hash: same-length replacement is rejected by the gateway.
    let (metadata, connection) = tokio_postgres::connect(&f.database, tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let row = metadata
        .query_one("SELECT body FROM archives WHERE id=$1", &[&id])
        .await
        .unwrap();
    let record: serde_json::Value = serde_json::from_str(row.get(0)).unwrap();
    let mut corrupt = bytes.clone();
    corrupt[5] ^= 1;
    s3.put_object()
        .bucket(&bucket)
        .key(record["receipt"]["objectKey"].as_str().unwrap())
        .body(aws_sdk_s3::primitives::ByteStream::from(corrupt))
        .send()
        .await
        .unwrap_or_else(|_| panic!("Owned corruption write failed; private diagnostics withheld"));
    let corrupted = restarted.plan("123", "124", id).await.unwrap();
    assert!(
        gateway
            .download(
                &corrupted,
                gateway.target(&corrupted, binding.clone()).unwrap(),
                Arc::new(Live {
                    engine: restarted.clone(),
                    a: authorization(&corrupted)
                })
            )
            .await
            .is_err()
    );
    // Simulate an already persisted, unqualified pre-upgrade PG receipt. The exact
    // existing object is retained for authorized recovery/deletion, not served.
    let mut legacy = record;
    legacy["kind"] = json!("postgres");
    legacy["source_binding"] = json!("a".repeat(64));
    metadata
        .execute(
            "UPDATE archives SET body=$2 WHERE id=$1",
            &[&id, &legacy.to_string()],
        )
        .await
        .unwrap();
    assert!(restarted.core_source("123", "124", id).await.is_err());
    assert!(restarted.available("123", "124", id).await.is_err());
    assert!(
        restarted
            .produce("123", "124", id, "job-one")
            .await
            .is_err()
    );
    assert!(restarted.plan("123", "124", id).await.is_err());
    let a = authorization(&p);
    assert!(
        restarted.authorize(&a).await.is_err(),
        "issued historical PG ticket denied before deletion"
    );
    restarted.delete("123", "124", id).await.unwrap();
    assert!(restarted.authorize(&a).await.is_err());
    assert!(restarted.plan("123", "124", id).await.is_err());
    restarted.delete("123", "124", id).await.unwrap();
    let id = uuid::Uuid::new_v4();
    let diagnostic = json!({"request_id":id,"kind":"diagnostics","retention_seconds":5,"diagnostics":{"version":1,"state":"finished","builds":[{"id":"123","outcome":"succeeded","duration_ms":11,"output_bytes":42}],"checks":[]}});
    restarted.create("123", "124", diagnostic).await.unwrap();
    restarted
        .produce("123", "124", id, "job-two")
        .await
        .unwrap();
    let p = restarted.plan("123", "124", id).await.unwrap();
    let a = authorization(&p);
    let stage = gateway
        .download(
            &p,
            gateway.target(&p, binding).unwrap(),
            Arc::new(Live {
                engine: restarted.clone(),
                a: a.clone(),
            }),
        )
        .await
        .unwrap();
    let mut stream = stage.into_stream();
    let mut compressed = Vec::new();
    while let Some(c) = stream.next_chunk().await.unwrap() {
        compressed.extend(c);
    }
    use std::io::Read;
    let mut output = String::new();
    flate2::read::GzDecoder::new(compressed.as_slice())
        .read_to_string(&mut output)
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&output).unwrap()["builds"][0]["output_bytes"],
        42
    );
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    assert!(restarted.authorize(&a).await.is_err());
    assert!(restarted.plan("123", "124", id).await.is_err());
    restarted.delete("123", "124", id).await.unwrap();
    // A missing source fails closed; no receipt is exposed, and deletion remains explicit.
    let id = uuid::Uuid::new_v4();
    assert!(
        restarted
            .create(
                "123",
                "125",
                json!({"request_id":id,"kind":"postgres","retention_seconds":3600})
            )
            .await
            .is_err()
    );
    assert!(
        restarted
            .produce("123", "125", id, "job-missing")
            .await
            .is_err()
    );
    assert!(restarted.plan("123", "125", id).await.is_err());

    assert_eq!(std::fs::read_dir(staging.path()).unwrap().count(), 0);
}
