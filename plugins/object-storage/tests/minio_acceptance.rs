//! Opt-in: only fresh generated buckets/users on an already running loopback MinIO.
//! No Docker dependency, global installs, ambient AWS configuration or provider errors in output.
#![cfg(feature = "providers")]
use aws_sdk_s3::{
    Client,
    config::{BehaviorVersion, Credentials, Region, retry::RetryConfig},
    primitives::ByteStream,
};
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, path::PathBuf, sync::Arc};
use webdock_object_storage_plugin::{
    engine::Engine,
    provider::Provider,
    sdk::{BackendConfig, Control},
    store::Store,
};
use webdock_plugin_kit::seal::Sealer;
mod support;

fn client(endpoint: &str, key: &str, secret: &str) -> Client {
    Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .endpoint_url(endpoint)
            .force_path_style(true)
            .retry_config(RetryConfig::disabled())
            .credentials_provider(Credentials::new(key, secret, None, None, "private-test"))
            .build(),
    )
}
async fn call(e: &Engine, action: &str, input: Value) -> Result<Value, String> {
    e.execute("acceptance-customer", "acceptance-project", action, input)
        .await
        .map_err(str::to_owned)
}
async fn change(e: &Engine, id: &str, action: &str, extra: Value) -> Result<Value, String> {
    let view = call(e, "buckets.view", json!({"resource_id":id})).await?;
    let mut input =
        json!({"resource_id":id,"request_id":uuid::Uuid::new_v4(),"revision":view["revision"]});
    input
        .as_object_mut()
        .ok_or("input")?
        .extend(extra.as_object().ok_or("extra")?.clone());
    call(e, action, input).await
}
async fn cleanup(e: &Engine, id: &str) -> Result<(), String> {
    let view = call(e, "buckets.view", json!({"resource_id":id})).await?;
    if view["state"] == "needs-reconciliation" {
        change(e, id, "buckets.reconcile", json!({})).await?;
    } else {
        change(e, id, "buckets.deactivate", json!({})).await?;
    }
    for _ in 0..40 {
        if change(e, id, "buckets.purge", json!({"confirm":id})).await?["purge_complete"] == true {
            change(e, id, "buckets.delete", json!({"confirm":id})).await?;
            return Ok(());
        }
    }
    Err("bounded cleanup exhausted".into())
}
// This helper is limited to three fixture bytes; production transfers use Core's binary transport.
async fn transfer(plan: &Value, method: &str, body: &[u8]) -> Result<Value, String> {
    use tokio::io::AsyncWriteExt;
    let mut child = tokio::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/support/transfer.py"
        ))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|_| "fixture transport start")?;
    let input = json!({"url":plan["url"],"headers":plan["headers"],"method":method,"body":hex::encode(body)});
    child
        .stdin
        .take()
        .ok_or("fixture input")?
        .write_all(&serde_json::to_vec(&input).map_err(|_| "fixture encoding")?)
        .await
        .map_err(|_| "fixture input write")?;
    let output = child
        .wait_with_output()
        .await
        .map_err(|_| "fixture transport wait")?;
    if !output.status.success() {
        return Err("fixture transport failed".into());
    }
    serde_json::from_slice(&output.stdout).map_err(|_| "fixture transport result".into())
}
async fn presigned_data(e: &Engine, id: &str, bucket: &str, admin: &Client) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let hash = hex::encode(Sha256::digest(b"abc"));
    let input = json!({"resource_id":id,"key":"data-plan-proof","exact_bytes":3,"expected_sha256":hash,"filename":"proof.txt","content_type":"text/plain","expires_at":webdock_object_storage_plugin::model::now()+120});
    let upload = call(e, "objects.upload", input.clone()).await?;
    let parsed = url::Url::parse(upload["url"].as_str().ok_or("private URL")?)
        .map_err(|_| "private URL parse")?;
    let signed_headers = parsed
        .query_pairs()
        .find(|(k, _)| k == "X-Amz-SignedHeaders")
        .map(|(_, v)| v.into_owned())
        .ok_or("missing signed headers")?;
    if !signed_headers.contains("content-length")
        || !signed_headers.contains("if-none-match")
        || !signed_headers.contains("x-amz-checksum-sha256")
    {
        return Err("upload size, checksum and create condition must all be signed".into());
    }
    if transfer(&upload, "GET", b"").await?["status"] == 200 {
        return Err("wrong signed method accepted".into());
    }
    if transfer(&upload, "PUT", b"abcd").await?["status"] == 200 {
        return Err("wrong signed length accepted".into());
    }
    let mut wrong_hash = upload.clone();
    wrong_hash["headers"]["x-amz-checksum-sha256"] =
        json!("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=");
    if transfer(&wrong_hash, "PUT", b"abc").await?["status"] == 200 {
        return Err("wrong signed checksum accepted".into());
    }
    let mut other = upload.clone();
    let mut substituted = parsed;
    substituted.set_path(&format!("/{bucket}/other-key"));
    other["url"] = json!(substituted.as_str());
    if transfer(&other, "PUT", b"abc").await?["status"] == 200 {
        return Err("object substitution accepted".into());
    }
    if transfer(&upload, "PUT", b"abc").await?["status"] != 200 {
        return Err("exact signed upload failed".into());
    }
    if transfer(&upload, "PUT", b"abc").await?["status"] != 412 {
        return Err("create-only overwrite accepted".into());
    }
    let head = admin
        .head_object()
        .bucket(bucket)
        .key("data-plan-proof")
        .checksum_mode(aws_sdk_s3::types::ChecksumMode::Enabled)
        .send()
        .await
        .map_err(|_| "fixture head")?;
    let mut download_input = input;
    download_input["version_id"] = json!(head.version_id().unwrap_or("null"));
    download_input["if_match"] = json!(head.e_tag().ok_or("fixture tag")?);
    let download = call(e, "objects.download", download_input.clone()).await?;
    if download["headers"]["if-match"] != download_input["if_match"] {
        return Err("download condition missing".into());
    }
    let response = transfer(&download, "GET", b"").await?;
    if response["status"] != 200 || response["body"] != "616263" {
        return Err("exact version download failed".into());
    }
    for (field, value) in [
        ("version_id", json!("missing-version")),
        ("if_match", json!("\"deadbeef\"")),
        ("exact_bytes", json!(4)),
        ("expected_sha256", json!("b".repeat(64))),
    ] {
        let mut bad = download_input.clone();
        bad[field] = value;
        if call(e, "objects.download", bad).await.is_ok() {
            return Err(format!("incorrect download {field} accepted"));
        }
    }
    // Remove this exact null version so existing inventory expectations stay unchanged.
    admin
        .delete_object()
        .bucket(bucket)
        .key("data-plan-proof")
        .version_id(head.version_id().unwrap_or("null"))
        .send()
        .await
        .map_err(|_| "fixture data cleanup")?;
    Ok(())
}
async fn scenario(
    e: &Engine,
    a: &str,
    b: &str,
    endpoint: &str,
    admin: &Client,
) -> Result<(), String> {
    let aview = call(e, "buckets.view", json!({"resource_id":a})).await?;
    let bview = call(e, "buckets.view", json!({"resource_id":b})).await?;
    let abucket = aview["bucket"].as_str().ok_or("bucket")?;
    let bbucket = bview["bucket"].as_str().ok_or("bucket")?;
    presigned_data(e, a, abucket, admin).await?;
    change(e, a, "versioning.set", json!({"enabled":true})).await?;
    let key = change(e, a, "credentials.create", json!({})).await?["access_key"]
        .as_str()
        .ok_or("key")?
        .to_owned();
    let secret = call(
        e,
        "credentials.reveal",
        json!({"resource_id":a,"access_key":key}),
    )
    .await?;
    let app = client(
        endpoint,
        &key,
        secret["secret_key"].as_str().ok_or("secret")?,
    );
    for body in ["old-version", "current-version"] {
        app.put_object()
            .bucket(abucket)
            .key("proof")
            .body(ByteStream::from_static(body.as_bytes()))
            .send()
            .await
            .map_err(|_| "app put failed")?;
    }
    if app
        .put_object()
        .bucket(bbucket)
        .key("foreign")
        .body(ByteStream::from_static(b"denied"))
        .send()
        .await
        .is_ok()
    {
        return Err("cross-bucket write allowed".into());
    }
    if let Ok(list) = app.list_buckets().send().await
        && list
            .buckets()
            .iter()
            .any(|bucket| bucket.name() != Some(abucket))
    {
        return Err("foreign bucket listing allowed".into());
    }
    if app
        .put_bucket_versioning()
        .bucket(abucket)
        .versioning_configuration(
            aws_sdk_s3::types::VersioningConfiguration::builder()
                .status(aws_sdk_s3::types::BucketVersioningStatus::Suspended)
                .build(),
        )
        .send()
        .await
        .is_ok()
    {
        return Err("app management allowed".into());
    }
    let upload = app
        .create_multipart_upload()
        .bucket(abucket)
        .key("unfinished")
        .send()
        .await
        .map_err(|_| "multipart start")?
        .upload_id()
        .ok_or("upload id")?
        .to_owned();
    app.upload_part()
        .bucket(abucket)
        .key("unfinished")
        .upload_id(&upload)
        .part_number(1)
        .body(ByteStream::from_static(b"part-bytes"))
        .send()
        .await
        .map_err(|_| "multipart part")?;
    // Exercise continuation across two uploads of one key and a different key.
    for key in ["unfinished", "another-unfinished"] {
        let upload = app
            .create_multipart_upload()
            .bucket(abucket)
            .key(key)
            .send()
            .await
            .map_err(|_| "multipart continuation start")?
            .upload_id()
            .ok_or("continuation upload id")?
            .to_owned();
        app.upload_part()
            .bucket(abucket)
            .key(key)
            .upload_id(upload)
            .part_number(1)
            .body(ByteStream::from_static(b"part-bytes"))
            .send()
            .await
            .map_err(|_| "multipart continuation part")?;
    }
    let mut inventory_complete = false;
    for _ in 0..40 {
        let v = change(e, a, "usage.inventory", json!({})).await?;
        if v["inventory"]["complete"] == true {
            if v["inventory"]["current_bytes"] != 15
                || v["inventory"]["noncurrent_bytes"] != 11
                || v["inventory"]["multipart_bytes"] != 30
                || v["inventory"]["multipart_uploads"] != 3
            {
                return Err("usage categories mismatch".into());
            }
            inventory_complete = true;
            break;
        }
    }
    if !inventory_complete {
        return Err("bounded inventory did not complete".into());
    }
    let new = change(e, a, "credentials.rotate", json!({})).await?["access_key"]
        .as_str()
        .ok_or("rotation key")?
        .to_owned();
    app.head_object()
        .bucket(abucket)
        .key("proof")
        .send()
        .await
        .map_err(|_| "overlap revoked too soon")?;
    change(e, a, "credentials.revoke", json!({"access_key":key})).await?;
    if app
        .head_object()
        .bucket(abucket)
        .key("proof")
        .send()
        .await
        .is_ok()
    {
        return Err("revoked key usable".into());
    }
    let rotated = call(
        e,
        "credentials.reveal",
        json!({"resource_id":a,"access_key":new}),
    )
    .await?;
    let app = client(
        endpoint,
        &new,
        rotated["secret_key"].as_str().ok_or("rotated secret")?,
    );
    if change(
        e,
        a,
        "lifecycle.set",
        json!({"policy":{"abort_multipart_days":7}}),
    )
    .await
    .is_ok()
    {
        return Err("unsupported local lifecycle accepted".into());
    }
    change(
        e,
        a,
        "lifecycle.set",
        json!({"policy":{"expire_current_days":30,"expire_noncurrent_days":40}}),
    )
    .await?;
    change(e, a, "buckets.deactivate", json!({})).await?;
    if app
        .head_object()
        .bucket(abucket)
        .key("proof")
        .send()
        .await
        .is_ok()
    {
        return Err("inactive key usable".into());
    }
    let retained = admin
        .get_object()
        .bucket(abucket)
        .key("proof")
        .send()
        .await
        .map_err(|_| "retained data unavailable")?
        .body
        .collect()
        .await
        .map_err(|_| "retained body")?
        .into_bytes();
    if retained.as_ref() != b"current-version" {
        return Err("data changed".into());
    }
    if change(e, a, "buckets.delete", json!({"confirm":a}))
        .await
        .is_ok()
    {
        return Err("nonempty deletion allowed".into());
    }
    change(e, a, "buckets.reactivate", json!({})).await?;
    if !call(e, "buckets.view", json!({"resource_id":a})).await?["credentials"]
        .as_array()
        .ok_or("credentials")?
        .is_empty()
    {
        return Err("keys resurrected".into());
    }
    paginated_inventory_and_mutations(e, b, admin, bbucket)
        .await
        .map_err(|error| format!("large fixture: {error}"))?;
    Ok(())
}

async fn scan(e: &Engine, id: &str) -> Result<Value, String> {
    for page in 0..160 {
        let v = change(e, id, "usage.inventory", json!({}))
            .await
            .map_err(|_| format!("large scan page {page} failed"))?;
        if v["inventory"]["complete"] == true {
            return Ok(v["inventory"].clone());
        }
    }
    Err("inventory page bound exhausted".into())
}
async fn paginated_inventory_and_mutations(
    e: &Engine,
    id: &str,
    admin: &Client,
    bucket: &str,
) -> Result<(), String> {
    // A null version predates enabling versioning; delete markers are separate records.
    admin
        .put_object()
        .bucket(bucket)
        .key("null-version")
        .body(ByteStream::from_static(b"null"))
        .send()
        .await
        .map_err(|_| "null put")?;
    change(e, id, "versioning.set", json!({"enabled":true})).await?;
    for _ in 0..105 {
        admin
            .put_object()
            .bucket(bucket)
            .key("versions")
            .body(ByteStream::from_static(b"v"))
            .send()
            .await
            .map_err(|_| "version put")?;
    }
    admin
        .delete_object()
        .bucket(bucket)
        .key("versions")
        .send()
        .await
        .map_err(|_| "delete marker")?;
    let mut uploads = vec![];
    for index in 0..101 {
        let key = format!("multipart-{index:03}");
        let upload = admin
            .create_multipart_upload()
            .bucket(bucket)
            .key(&key)
            .send()
            .await
            .map_err(|_| "paginated upload")?
            .upload_id()
            .ok_or("paginated upload id")?
            .to_owned();
        for part in 1..=if index == 0 { 101 } else { 1 } {
            admin
                .upload_part()
                .bucket(bucket)
                .key(&key)
                .upload_id(&upload)
                .part_number(part)
                .body(ByteStream::from_static(b"p"))
                .send()
                .await
                .map_err(|_| "paginated part")?;
        }
        uploads.push((key, upload));
    }
    let listed_parts = admin
        .list_parts()
        .bucket(bucket)
        .key(&uploads[0].0)
        .upload_id(&uploads[0].1)
        .max_parts(100)
        .send()
        .await
        .map_err(|_| "direct parts discovery failed")?;
    if listed_parts.parts().len() != 100
        || listed_parts.is_truncated() != Some(true)
        || listed_parts.next_part_number_marker() != Some("100")
    {
        return Err(format!(
            "unexpected parts page: count={}, truncated={:?}, marker={:?}",
            listed_parts.parts().len(),
            listed_parts.is_truncated(),
            listed_parts.next_part_number_marker()
        ));
    }
    let totals = scan(e, id).await?;
    if totals["current_bytes"] != 4
        || totals["noncurrent_bytes"] != 105
        || totals["noncurrent_versions"] != 105
        || totals["delete_markers"] != 1
        || totals["multipart_uploads"] != 101
        || totals["multipart_bytes"] != 201
    {
        return Err("large pagination totals mismatch".into());
    }
    // Capture a persisted ListParts continuation, then abort that upload externally.
    for _ in 0..3 {
        change(e, id, "usage.inventory", json!({})).await?;
    }
    admin
        .abort_multipart_upload()
        .bucket(bucket)
        .key(&uploads[0].0)
        .upload_id(&uploads[0].1)
        .send()
        .await
        .map_err(|_| "concurrent abort")?;
    let reset = change(e, id, "usage.inventory", json!({})).await?;
    if reset["scan_reset"] != true || !reset["inventory"].is_null() || reset["state"] != "active" {
        return Err("concurrent multipart scan not reset".into());
    }
    // A concurrent completed upload invalidates the buffered discovery ID as well.
    for _ in 0..3 {
        change(e, id, "usage.inventory", json!({})).await?;
    }
    let completed = &uploads[2];
    let parts = admin
        .list_parts()
        .bucket(bucket)
        .key(&completed.0)
        .upload_id(&completed.1)
        .send()
        .await
        .map_err(|_| "completion part")?;
    let tag = parts
        .parts()
        .first()
        .and_then(|p| p.e_tag())
        .ok_or("part tag")?;
    admin
        .complete_multipart_upload()
        .bucket(bucket)
        .key(&completed.0)
        .upload_id(&completed.1)
        .multipart_upload(
            aws_sdk_s3::types::CompletedMultipartUpload::builder()
                .parts(
                    aws_sdk_s3::types::CompletedPart::builder()
                        .part_number(1)
                        .e_tag(tag)
                        .build(),
                )
                .build(),
        )
        .send()
        .await
        .map_err(|_| "concurrent completion")?;
    let reset = change(e, id, "usage.inventory", json!({})).await?;
    if reset["scan_reset"] != true || reset["state"] != "active" {
        return Err("completed upload scan not reset".into());
    }
    // Mutate versions after the first page: the current scan is explicitly non-atomic,
    // and a fresh scan must settle to accurate totals without resource reconciliation.
    change(e, id, "usage.inventory", json!({})).await?;
    admin
        .delete_object()
        .bucket(bucket)
        .key("null-version")
        .version_id("null")
        .send()
        .await
        .map_err(|_| "concurrent null delete")?;
    admin
        .put_object()
        .bucket(bucket)
        .key("concurrent")
        .body(ByteStream::from_static(b"new"))
        .send()
        .await
        .map_err(|_| "concurrent object put")?;
    scan(e, id).await?;
    let settled = scan(e, id).await?;
    if settled["current_bytes"] != 4
        || settled["noncurrent_bytes"] != 105
        || settled["multipart_uploads"] != 99
        || settled["multipart_bytes"] != 99
        || settled["delete_markers"] != 1
    {
        return Err("settled concurrent scan mismatch".into());
    }
    change(e, id, "buckets.deactivate", json!({})).await?;
    let partial = change(e, id, "buckets.purge", json!({"confirm":id})).await?;
    if partial["purge_complete"] != false {
        return Err("partial purge reported complete".into());
    }
    Ok(())
}
#[tokio::test]
#[ignore = "requires WEBDOCK_OBJECT_STORAGE_MINIO_CONFIG private file; only fresh owned fixture resources"]
async fn real_minio_lifecycle_isolation_versions_multipart_retention_and_exact_cleanup() {
    let path = PathBuf::from(
        std::env::var_os("WEBDOCK_OBJECT_STORAGE_MINIO_CONFIG")
            .expect("private local config path required"),
    );
    let bytes = webdock_plugin_kit::read_file(&path, true).expect("private config");
    let raw: Value = serde_json::from_slice(&bytes).expect("config JSON");
    assert_eq!(raw["kind"], "minio-local");
    let c: BackendConfig = serde_json::from_slice(&bytes).expect("explicit config");
    let endpoint = c.endpoint.clone();
    let admin = client(&endpoint, &c.access_key, &c.secret_key);
    let provider = Arc::new(Control::new(c).await.expect("local control config"));
    assert!(provider.endpoint().starts_with("http://127.0.0.1:"));
    let directory = path
        .parent()
        .expect("private parent")
        .join(format!("acceptance-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    let encryption_key = hex::encode(webdock_plugin_kit::seal::random::<32>());
    std::fs::write(directory.join("sealer-key"), &encryption_key).unwrap();
    std::fs::set_permissions(
        directory.join("sealer-key"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let db = support::Database::new().await;
    let storage = directory.join("storage.json");
    std::fs::write(
        &storage,
        json!({"url":db.url,"database":db.name}).to_string(),
    )
    .unwrap();
    std::fs::set_permissions(&storage, std::fs::Permissions::from_mode(0o600)).unwrap();
    let e = Engine::new(
        db.store().await,
        Sealer::new(&encryption_key).unwrap(),
        provider,
    );
    let mut ids = Vec::new();
    let mut result = Ok(());
    for name in ["fixture-a", "fixture-b"] {
        match call(
            &e,
            "buckets.create",
            json!({"request_id":uuid::Uuid::new_v4(),"name":name,"environment":"acceptance"}),
        )
        .await
        {
            Ok(v) => ids.push(v["resource"]["id"].as_str().unwrap().to_owned()),
            Err(error) => {
                result = Err(format!("provision: {error}"));
                break;
            }
        }
    }
    if ids.len() == 2 {
        result = scenario(&e, &ids[0], &ids[1], &endpoint, &admin).await;
    }
    let mut cleanup_errors = 0;
    for id in &ids {
        if cleanup(&e, id).await.is_err() {
            cleanup_errors += 1;
        }
    }
    // Never print raw SDK/CLI errors or configuration. Retain exact private metadata on failures.
    assert_eq!(
        cleanup_errors,
        0,
        "exact fixture cleanup incomplete; scenario={:?}; inspect private metadata",
        result.as_ref().err()
    );
    if let Err(error) = result {
        panic!("MinIO acceptance failed: {error}; private metadata retained");
    }
    assert_eq!(ids.len(), 2);
    assert_eq!(
        db.client()
            .await
            .query_one("SELECT count(*) FROM resources", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        2
    );
    std::fs::remove_dir_all(&directory).unwrap();
}

#[tokio::test]
#[ignore = "explicit recovery of one exact private acceptance directory only"]
async fn recover_exact_owned_fixture() {
    let Some(path) = std::env::var_os("WEBDOCK_OBJECT_STORAGE_RECOVERY") else {
        return;
    };
    let path = PathBuf::from(path);
    let config = PathBuf::from(
        std::env::var_os("WEBDOCK_OBJECT_STORAGE_MINIO_CONFIG").expect("private config"),
    );
    assert_eq!(path.parent(), config.parent());
    let bytes = webdock_plugin_kit::read_file(&config, true).unwrap();
    let c: BackendConfig = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(c.kind, "minio-local");
    let provider = Arc::new(Control::new(c).await.unwrap());
    let storage: Value = serde_json::from_slice(
        &webdock_plugin_kit::read_file(&path.join("storage.json"), true).unwrap(),
    )
    .unwrap();
    let url = storage["url"].as_str().unwrap();
    let connection = webdock_plugin_kit::postgres::connect(url, None)
        .await
        .expect("private fixture metadata");
    let resources = connection
        .query("SELECT body FROM resources", &[])
        .await
        .unwrap()
        .iter()
        .map(|row| {
            serde_json::from_str::<webdock_object_storage_plugin::model::Resource>(
                row.get::<_, &str>(0),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    drop(connection);
    for r in &resources {
        assert_eq!(r.binding.customer, "acceptance-customer");
        assert_eq!(r.binding.project, "acceptance-project");
        assert_eq!(r.binding.environment, "acceptance");
        assert_eq!(r.binding.backend, provider.backend());
        if !r.provisioned {
            let observed = provider.observe(&r.binding).await.unwrap();
            if observed.exists {
                // Explicit cleanup only: match this fixture's tagged bucket and private generated policy.
                // Never promote an interrupted resource to active or adopt an untagged collision.
                assert!(observed.owned);
                let raw: Value = serde_json::from_slice(&bytes).unwrap();
                let policy = PathBuf::from(raw["minio"]["configDir"].as_str().unwrap())
                    .join(format!("{}.policy.json", r.binding.id.simple()));
                let saved: Value =
                    serde_json::from_slice(&webdock_plugin_kit::read_file(&policy, true).unwrap())
                        .unwrap();
                assert_eq!(
                    saved,
                    webdock_object_storage_plugin::sdk::application_policy(&r.binding)
                );
                provider.set_lifecycle(&r.binding, None).await.unwrap();
                for key in provider.keys(&r.binding).await.unwrap() {
                    provider.revoke_key(&r.binding, &key).await.unwrap();
                }
                assert!(bounded_fixture_purge(&provider, &r.binding).await);
                provider.delete_empty(&r.binding).await.unwrap();
            }
        }
    }
    if resources
        .iter()
        .any(|r| r.provisioned && r.state != "deleted")
    {
        let key = String::from_utf8(
            webdock_plugin_kit::read_file(&path.join("sealer-key"), true).unwrap(),
        )
        .unwrap();
        let e = Engine::new(
            Store::connect(url, None).await.unwrap(),
            Sealer::new(&key).unwrap(),
            provider.clone(),
        );
        for r in resources
            .iter()
            .filter(|r| r.provisioned && r.state != "deleted")
        {
            if r.pending.as_deref() == Some("lifecycle.set") {
                // Explicit operator recovery of the observed MinIO partial-policy qualification failure.
                let observed = provider.observe(&r.binding).await.unwrap();
                assert!(observed.owned);
                for key in provider.keys(&r.binding).await.unwrap() {
                    provider.revoke_key(&r.binding, &key).await.unwrap();
                }
                provider.set_lifecycle(&r.binding, None).await.unwrap();
                assert!(provider.keys(&r.binding).await.unwrap().is_empty());
                assert!(
                    provider
                        .observe(&r.binding)
                        .await
                        .unwrap()
                        .lifecycle
                        .is_none()
                );
                let fence = e.store().lock().await.unwrap();
                let mut repaired = r.clone();
                repaired.state = "inactive".into();
                repaired.pending = None;
                repaired.credentials.clear();
                repaired.revision += 1;
                fence.save(&repaired).await.unwrap();
                drop(fence);
                support::released(e.store()).await;
            }
            cleanup(&e, &r.binding.id.to_string())
                .await
                .expect("exact owned recovery cleanup");
        }
    }
    for r in &resources {
        assert!(!provider.observe(&r.binding).await.unwrap().exists);
    }
    std::fs::remove_dir_all(path).unwrap();
}

async fn bounded_fixture_purge(
    provider: &Control,
    binding: &webdock_object_storage_plugin::model::Binding,
) -> bool {
    for _ in 0..40 {
        if provider.purge(binding).await.unwrap() {
            return true;
        }
    }
    false
}
