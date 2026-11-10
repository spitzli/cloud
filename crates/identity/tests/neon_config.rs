use spitzli_identity::NeonAuthClient;
#[test]
fn managed_auth_has_one_fixed_tls_endpoint_and_trusted_application_origin() {
    assert!(
        NeonAuthClient::new(
            "https://ep-owned.neonauth.c-7.eu-central-1.aws.neon.tech/neondb/auth",
            "https://auth.webdock.dev"
        )
        .is_ok()
    );
    for base in [
        "http://ep.neonauth.eu.neon.tech/neondb/auth",
        "https://foreign.invalid/neondb/auth",
        "https://u:pw@ep.neonauth.eu.neon.tech/neondb/auth",
        "https://ep.neonauth.eu.neon.tech/neondb/auth?next=evil",
        "https://ep.neonauth.eu.neon.tech/other",
    ] {
        assert!(NeonAuthClient::new(base, "https://auth.webdock.dev").is_err());
    }
    assert!(
        NeonAuthClient::new(
            "https://ep.neonauth.eu.neon.tech/neondb/auth",
            "https://auth.webdock.dev/path"
        )
        .is_err()
    );
}
