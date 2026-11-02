#![cfg(feature = "providers")]
use webdock_object_storage_plugin::model::Binding;
use webdock_object_storage_plugin::sdk::{BackendConfig, Control, application_policy};
#[tokio::test]
async fn endpoints_and_credentials_are_explicit_and_minio_is_loopback_only() {
    for endpoint in [
        "https://s3.eu-central-2.wasabisys.com",
        "https://s3.wasabisys.com",
    ] {
        let config = serde_json::json!({"id":"wasabi-trial","endpoint":endpoint,"region":"eu-central-2","account":"private-account-binding","accessKey":"explicit","secretKey":"explicit-secret","kind":"wasabi"});
        let c: BackendConfig = serde_json::from_value(config).unwrap();
        assert!(Control::new(c).await.is_ok());
    }
    for endpoint in [
        "http://s3.wasabisys.com",
        "https://evil.example",
        "https://s3.wasabisys.com/path",
        "https://key:secret@s3.wasabisys.com",
    ] {
        let c:BackendConfig=serde_json::from_value(serde_json::json!({"id":"bad","endpoint":endpoint,"region":"eu-central-2","account":"x","accessKey":"explicit","secretKey":"explicit-secret","kind":"wasabi"})).unwrap();
        assert!(Control::new(c).await.is_err());
    }
    let missing = serde_json::json!({"id":"bad","endpoint":"https://s3.wasabisys.com","region":"eu-central-2","account":"x","kind":"wasabi"});
    assert!(serde_json::from_value::<BackendConfig>(missing).is_err());
}
#[test]
fn application_policy_has_only_exact_bucket_data_authority() {
    let id = uuid::Uuid::new_v4();
    let b = Binding {
        id,
        customer: "a".into(),
        project: "p".into(),
        environment: "prod".into(),
        backend: "test".into(),
        bucket: format!("wd-{}", id.simple()),
        user: format!("wd-{}", id.simple()),
    };
    let p = application_policy(&b);
    assert_eq!(
        p["Statement"][0]["Resource"],
        format!("arn:aws:s3:::{}", b.bucket)
    );
    assert_eq!(
        p["Statement"][1]["Resource"],
        format!("arn:aws:s3:::{}/*", b.bucket)
    );
    let text = p.to_string();
    assert!(!text.contains("s3:*"));
    assert!(!text.contains("iam:"));
    assert!(!text.contains("PutBucket"));
    assert!(!text.contains("PutObjectAcl"));
}
