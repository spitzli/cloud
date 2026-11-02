//! The real native feature closure combines AWS's aws-lc and tonic's ring TLS support.
#![cfg(feature = "providers")]

#[tokio::test]
async fn sdk_initialization_selects_a_provider_before_tonic_tls_configuration() {
    use webdock_object_storage_plugin::sdk::{BackendConfig, Control};
    assert!(rustls::crypto::CryptoProvider::get_default().is_none());
    // Client construction does no provider I/O; neither an AWS config loader nor a request is used.
    let config: BackendConfig = serde_json::from_value(serde_json::json!({
        "id":"startup-test", "kind":"wasabi", "endpoint":"https://s3.wasabisys.com",
        "region":"us-east-1", "account":"unconnected-test", "accessKey":"test-only", "secretKey":"test-only"
    })).unwrap();
    assert!(Control::new(config).await.is_ok());
    assert!(rustls::crypto::CryptoProvider::get_default().is_some());
    assert!(
        tonic::transport::Endpoint::from_static("https://localhost:1")
            .tls_config(tonic::transport::ClientTlsConfig::new())
            .is_ok()
    );
}
