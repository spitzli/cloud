use webdock_mail_protocol::native_control::NativeControlClient;
use webdock_mail_runtime::SendControl;
#[tokio::test]
async fn native_client_is_send_control_and_preserves_safe_status() {
    let channel = tonic::transport::Endpoint::from_static("http://127.0.0.1:1").connect_lazy();
    let client = NativeControlClient::new(channel, 1).unwrap();
    let control: &dyn SendControl = &client;
    assert_eq!(
        control
            .call(serde_json::json!([]))
            .await
            .unwrap_err()
            .status,
        400
    );
}
