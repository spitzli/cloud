#![cfg(all(feature = "kubernetes-job", feature = "worker"))]
use chrono::{Duration, Utc};
use http::{Request, Response};
use http_body_util::{BodyExt, Full};
use kube::{Client, client::Body};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tower::service_fn;
use webdock_agent_protocol::build_storage::{
    BuildStorageAction, BuildStorageReservation, BuildStorageTarget, BuildVolumeFailure,
    BuildVolumeReply,
};
use webdock_build::build_storage::KubernetesBuildStorage;

#[tokio::test]
async fn request_is_idempotent_reply_binds_original_uid_and_cleanup_cannot_delete_replacement() {
    let maps = Arc::new(Mutex::new(
        std::collections::BTreeMap::<String, Value>::new(),
    ));
    let remote = maps.clone();
    let client = Client::new(
        service_fn(move |request: Request<Body>| {
            let remote = remote.clone();
            async move {
                let method = request.method().to_string();
                let path = request.uri().path().to_owned();
                let bytes = request.into_body().collect().await.unwrap().to_bytes();
                let mut body: Value = if bytes.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes).unwrap()
                };
                let mut maps = remote.lock().unwrap();
                let (status, value) = if method == "POST" {
                    let key = format!("{}/{}", path, body["metadata"]["name"].as_str().unwrap());
                    body["metadata"]["uid"] = json!("request-uid-1");
                    body["metadata"]["resourceVersion"] = json!("1");
                    maps.insert(key, body.clone());
                    (201, body)
                } else if method == "DELETE" {
                    let current = maps.get(&path).unwrap();
                    assert_eq!(body["preconditions"]["uid"], current["metadata"]["uid"]);
                    assert_eq!(
                        body["preconditions"]["resourceVersion"],
                        current["metadata"]["resourceVersion"]
                    );
                    maps.remove(&path);
                    (
                        200,
                        json!({"apiVersion":"v1","kind":"Status","status":"Success","code":200}),
                    )
                } else if let Some(value) = maps.get(&path) {
                    (200, value.clone())
                } else {
                    (
                        404,
                        json!({"apiVersion":"v1","kind":"Status","code":404,"reason":"NotFound","message":"owned absence"}),
                    )
                };
                Ok::<_, std::convert::Infallible>(
                    Response::builder()
                        .status(status)
                        .header("content-type", "application/json")
                        .body(Full::new(serde_json::to_vec(&value).unwrap().into()))
                        .unwrap(),
                )
            }
        }),
        "default",
    );
    let target = BuildStorageTarget {
        cluster_id: "78".into(),
        node: "build-node-1".into(),
        pool_id: "123e4567-e89b-42d3-a456-426614174000".into(),
    };
    let storage = KubernetesBuildStorage::new(
        client,
        "webdock-build-qualification".into(),
        "webdock-build-storage-requests".into(),
        "webdock-build-storage-receipts".into(),
        target.clone(),
        ed25519_dalek::SigningKey::from_bytes(&[24; 32]),
    )
    .unwrap();
    let request = BuildStorageReservation {
        version: 1,
        audience: "webdock-build-storage-v1".into(),
        volume_id: "1234".into(),
        customer_id: "12".into(),
        project_id: "34".into(),
        build_id: "56".into(),
        generation: 1,
        target,
        volume_bytes: 67108864,
        action: BuildStorageAction::Reserve,
        epoch: 1,
        expires_at: Utc::now() + Duration::seconds(120),
    };
    let mut operation = storage.prepare(request.clone()).unwrap();
    storage.send(&mut operation).await.unwrap();
    storage.send(&mut operation).await.unwrap();
    assert_eq!(maps.lock().unwrap().len(), 1);
    assert_eq!(operation.request_uid.as_deref(), Some("request-uid-1"));
    assert!(storage.reply(&operation).await.unwrap().is_none());
    let name = request.queue_name().unwrap();
    let reply_path = format!("/api/v1/namespaces/webdock-build-storage-receipts/configmaps/{name}");
    let mut reply = BuildVolumeReply {
        version: 1,
        request_uid: "replacement".into(),
        request_fingerprint: request.fingerprint().unwrap(),
        result: None,
        failure_code: Some(BuildVolumeFailure::Expired),
    };
    let mut object = json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":name,"namespace":"webdock-build-storage-receipts","uid":"reply-uid-1","resourceVersion":"2","labels":{"webdock.dev/component":"build-volume-reply","webdock.dev/volume-id":"1234"},"annotations":{"webdock.dev/request-sha256":request.fingerprint().unwrap()}},"immutable":true,"data":{"reply.json":serde_json::to_string(&reply).unwrap()}});
    maps.lock()
        .unwrap()
        .insert(reply_path.clone(), object.clone());
    assert!(storage.reply(&operation).await.is_err());
    reply.request_uid = "request-uid-1".into();
    object["data"]["reply.json"] = json!(serde_json::to_string(&reply).unwrap());
    maps.lock().unwrap().insert(reply_path.clone(), object);
    assert!(
        storage
            .reply(&operation)
            .await
            .unwrap()
            .unwrap()
            .failure_code
            .is_some()
    );
    let request_path = maps
        .lock()
        .unwrap()
        .keys()
        .find(|p| p.contains("storage-requests"))
        .unwrap()
        .clone();
    maps.lock().unwrap().get_mut(&request_path).unwrap()["metadata"]["uid"] = json!("replacement");
    assert!(storage.cleanup(&operation).await.is_err());
    assert_eq!(maps.lock().unwrap().len(), 2);
    maps.lock().unwrap().get_mut(&request_path).unwrap()["metadata"]["uid"] =
        json!("request-uid-1");
    storage.cleanup(&operation).await.unwrap();
    assert!(maps.lock().unwrap().is_empty());
}
