//! The Mail fixture must satisfy the real privileged helper's image policy.
#[test]
fn mail_postgres_target_uses_the_existing_fully_qualified_digest_policy() {
    let fixture = include_str!("../../control-plane/tests/support/recovery_mail_component.rs");
    let image = fixture
        .split("const PG_IMAGE: &str =")
        .nth(1)
        .unwrap()
        .split('"')
        .nth(1)
        .unwrap();
    assert!(
        webdock_supervisor::image_digest(image),
        "a shorthand Docker reference cannot enter privileged recovery trust"
    );
    assert_eq!(
        image,
        "docker.io/library/postgres@sha256:4da1a4828be12604092fa55311276f08f9224a74a62dcb4708bd7439e2a03911"
    );
    assert!(!webdock_supervisor::image_digest(
        "postgres@sha256:4da1a4828be12604092fa55311276f08f9224a74a62dcb4708bd7439e2a03911"
    ));
}
