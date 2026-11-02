use webdock_mail::gateway_control::HttpControl;
#[test]
fn control_requires_canonical_tls_auth_and_same_pod_submission_endpoint() {
    let secret = "x".repeat(32);
    assert!(
        HttpControl::new(
            "https://auth.example.test",
            "http://127.0.0.1:3139",
            &secret,
            &[]
        )
        .is_ok()
    );
    for origin in [
        "http://auth.example.test",
        "https://user:pass@auth.example.test",
        "https://auth.example.test/path",
        "https://auth.example.test?x=y",
    ] {
        assert!(HttpControl::new(origin, "http://127.0.0.1:3139", &secret, &[]).is_err());
    }
    for origin in [
        "https://send.example.test",
        "http://localhost:3139",
        "http://127.0.0.1:3139/path",
        "http://user:pass@127.0.0.1:3139",
    ] {
        assert!(HttpControl::new("https://auth.example.test", origin, &secret, &[]).is_err());
    }
    assert!(
        HttpControl::new(
            "https://auth.example.test",
            "http://127.0.0.1:3139",
            "short",
            &[]
        )
        .is_err()
    );
}
