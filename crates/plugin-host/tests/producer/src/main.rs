//! Actual producer Engine + AWS SDK presigns through Core's binary kernel, fresh resources only.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{os::unix::fs::PermissionsExt, sync::Arc};
use webdock_object_storage_plugin::{
    engine::Engine,
    sdk::{BackendConfig, Control},
    store::Store,
};
use webdock_plugin_host::storage::{StorageAuthority, StorageGateway};
use webdock_plugin_http::storage::StorageBinding;
use webdock_plugin_protocol::{
    PluginError,
    storage::{StorageAuthorization, StoragePlan},
};

struct Live {
    engine: Arc<Engine>,
    authorization: StorageAuthorization,
}
#[async_trait::async_trait]
impl StorageAuthority for Live {
    async fn check(&self) -> Result<(), PluginError> {
        self.engine
            .authorize_data(&self.authorization)
            .await
            .map_err(|_| PluginError::denied())
    }
}
async fn call(e: &Engine, action: &str, input: Value) -> Result<Value, &'static str> {
    e.execute("123", "124", action, input).await
}
async fn change(e: &Engine, id: &str, action: &str, extra: Value) -> Result<Value, &'static str> {
    let view = call(e, "buckets.view", json!({"resource_id":id})).await?;
    let mut input =
        json!({"resource_id":id,"request_id":uuid::Uuid::new_v4(),"revision":view["revision"]});
    input
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    call(e, action, input).await
}
fn live(e: Arc<Engine>, p: &StoragePlan) -> Arc<Live> {
    Arc::new(Live {
        engine: e,
        authorization: StorageAuthorization {
            version: 1,
            transfer_id: uuid::Uuid::new_v4().to_string(),
            source_invocation: uuid::Uuid::new_v4().to_string(),
            plugin_id: "object-storage".into(),
            generation: 1,
            action: if p.direction.method() == "PUT" {
                "objects.upload"
            } else {
                "objects.download"
            }
            .into(),
            customer_id: "123".into(),
            project_id: "124".into(),
            resource_id: p.resource_id.clone(),
            resource_epoch: p.resource_epoch.clone(),
            direction: p.direction,
            exact_bytes: p.exact_bytes,
            expected_sha256: p.expected_sha256.clone(),
            expires_at: p.expires_at,
        },
    })
}
async fn scenario(
    e: Arc<Engine>,
    id: &str,
    bucket: &str,
    admin: aws_sdk_s3::Client,
    origin: &str,
) -> Result<(), &'static str> {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .map_err(|_| "staging directory")?;
    let gateway = StorageGateway::loopback_test(directory.path().into()).map_err(|_| "gateway")?;
    let binding = StorageBinding {
        origin: origin.into(),
        addresses: vec!["127.0.0.1".parse().unwrap()],
        ca: None,
        path_prefix: format!("/{bucket}/"),
    };
    let bytes = vec![163u8; 3 * 1024 * 1024 + 17];
    let input = json!({"resource_id":id,"key":"gateway-proof","exact_bytes":bytes.len(),"expected_sha256":hex::encode(Sha256::digest(&bytes)),"filename":"proof.bin","content_type":"application/octet-stream","expires_at":webdock_object_storage_plugin::model::now()+240});
    let p: StoragePlan = serde_json::from_value(call(&e, "objects.upload", input.clone()).await?)
        .map_err(|_| "upload plan")?;
    let authority = live(e.clone(), &p);
    let stage = || {
        gateway.stage(
            &p,
            futures_util::stream::iter(bytes.chunks(65536).map(|c| Ok(c.to_vec()))),
            authority.clone(),
        )
    };
    let receipt = gateway
        .upload(
            &p,
            gateway
                .target(&p, binding.clone())
                .map_err(|_| "SDK upload target rejected")?,
            stage().await.map_err(|_| "upload stage")?,
        )
        .await
        .map_err(|_| "SDK conditional upload")?;
    assert_eq!(receipt.sha256, input["expected_sha256"].as_str().unwrap());
    // Replaying the exact AWS conditional presign cannot overwrite the object.
    assert!(
        gateway
            .upload(
                &p,
                gateway
                    .target(&p, binding.clone())
                    .map_err(|_| "replay target")?,
                stage().await.map_err(|_| "replay stage")?
            )
            .await
            .is_err()
    );
    let head = admin
        .head_object()
        .bucket(bucket)
        .key("gateway-proof")
        .checksum_mode(aws_sdk_s3::types::ChecksumMode::Enabled)
        .send()
        .await
        .map_err(|_| "owned object head")?;
    let mut download = input;
    download["version_id"] = json!(head.version_id().unwrap_or("null"));
    download["if_match"] = json!(head.e_tag().ok_or("owned ETag")?);
    let p: StoragePlan = serde_json::from_value(call(&e, "objects.download", download).await?)
        .map_err(|_| "download plan")?;
    let mut stream = gateway
        .download(
            &p,
            gateway
                .target(&p, binding)
                .map_err(|_| "SDK download target rejected")?,
            live(e.clone(), &p),
        )
        .await
        .map_err(|_| "SDK conditional download")?
        .into_stream();
    let mut hash = Sha256::new();
    let mut count = 0;
    while let Some(chunk) = stream.next_chunk().await.map_err(|_| "delivery")? {
        count += chunk.len();
        hash.update(chunk);
    }
    assert_eq!(count, bytes.len());
    assert_eq!(hex::encode(hash.finalize()), receipt.sha256);
    drop(stream);
    change(&e, id, "buckets.deactivate", json!({})).await?;
    assert!(
        live(e, &p).check().await.is_err(),
        "actual producer private state revokes epoch"
    );
    Ok(())
}
async fn run() -> Result<(), &'static str> {
    // Configuration paths/URL arrive privately on stdin; never echo them or provider errors.
    let config: Value =
        serde_json::from_reader(std::io::stdin()).map_err(|_| "private fixture input")?;
    let bytes = webdock_plugin_kit::read_file(
        std::path::Path::new(config["backend"].as_str().ok_or("backend path")?),
        true,
    )
    .map_err(|_| "private backend")?;
    let backend: BackendConfig = serde_json::from_slice(&bytes).map_err(|_| "backend contract")?;
    if backend.kind != "minio-local" {
        return Err("loopback MinIO required");
    }
    let endpoint = backend.endpoint.clone();
    let admin = aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new(backend.region.clone()))
            .endpoint_url(&endpoint)
            .force_path_style(true)
            .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
            .credentials_provider(aws_sdk_s3::config::Credentials::new(
                backend.access_key.clone(),
                backend.secret_key.clone(),
                None,
                None,
                "owned-fixture",
            ))
            .build(),
    );
    let engine = Arc::new(Engine::new(
        Store::connect(config["database"].as_str().ok_or("database input")?, None).await?,
        webdock_plugin_kit::seal::Sealer::new(&hex::encode(
            webdock_plugin_kit::seal::random::<32>(),
        ))?,
        Arc::new(Control::new(backend).await?),
    ));
    let created=call(&engine,"buckets.create",json!({"request_id":uuid::Uuid::new_v4(),"name":"gateway-fixture","environment":"acceptance"})).await?;
    let id = created["resource"]["id"].as_str().ok_or("created id")?;
    let view = call(&engine, "buckets.view", json!({"resource_id":id})).await?;
    let result = scenario(
        engine.clone(),
        id,
        view["bucket"].as_str().ok_or("owned bucket")?,
        admin,
        &endpoint,
    )
    .await;
    if call(&engine, "buckets.view", json!({"resource_id":id})).await?["state"] == "active" {
        change(&engine, id, "buckets.deactivate", json!({})).await?;
    }
    let mut purged = false;
    for _ in 0..40 {
        if change(&engine, id, "buckets.purge", json!({"confirm":id})).await?["purge_complete"]
            == true
        {
            purged = true;
            break;
        }
    }
    if !purged {
        return Err("owned purge incomplete");
    }
    change(&engine, id, "buckets.delete", json!({"confirm":id})).await?;
    result
}
#[tokio::main]
async fn main() {
    // Suppress panic diagnostics: assertions must never include a signed request or credential.
    std::panic::set_hook(Box::new(|_| {
        eprintln!("owned producer fixture assertion failed; private details withheld")
    }));
    match run().await {
        Ok(()) => println!(
            "Actual producer + gateway: SDK conditional PUT/GET, SHA256, 3MiB binary, overwrite rejection, private revocation and exact cleanup passed."
        ),
        Err(stage) => {
            eprintln!("owned producer fixture failed: {stage}");
            std::process::exit(1);
        }
    }
}
