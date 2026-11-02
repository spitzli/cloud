use webdock_oauth::{OAuthConfig, legacy_token_hash};
#[test]
fn issuer_resource_configuration_and_legacy_hash_are_exact() {
    assert_eq!(
        legacy_token_hash("fixture-token"),
        "0H-WPCu3o8x0qRDlHKAHWxlNpBKpNzeIHxBaVRVqmI8"
    );
    assert!(
        OAuthConfig::new(
            "https://auth.example.invalid/api/auth",
            "https://studio.example.invalid/api/mcp",
            None
        )
        .is_ok()
    );
    for issuer in [
        "http://auth.example.invalid/api/auth",
        "https://auth.example.invalid/api/auth?x=1",
        "https://auth.example.invalid/other",
        "https://user:password@auth.example.invalid/api/auth",
    ] {
        assert!(OAuthConfig::new(issuer, "https://studio.example.invalid/api/mcp", None).is_err());
    }
}
