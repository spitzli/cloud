use webdock_access::{CustomerType, TenantProfileInput, website_url};
#[test]
fn profile_and_site_boundary_preserve_existing_contracts() {
    let profile = TenantProfileInput {
        customer_type: CustomerType::Company,
        name: " Fixture ".into(),
        country: Some("de".into()),
        ..Default::default()
    }
    .validate()
    .unwrap();
    assert_eq!(profile.name, "Fixture");
    assert_eq!(profile.country.as_deref(), Some("DE"));
    assert!(
        TenantProfileInput {
            name: "x\u{0}".into(),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        TenantProfileInput {
            name: "Fixture".into(),
            country: Some("USA".into()),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert_eq!(
        website_url(
            &["https://customer.example.invalid/api/sso/callback".into()],
            &[],
            false
        )
        .unwrap(),
        "https://customer.example.invalid/cms"
    );
    assert!(
        website_url(
            &["https://studio.webdock.dev/api/sso/callback".into()],
            &[],
            false
        )
        .is_none()
    );
    assert!(
        website_url(
            &["https://user:secret@customer.example.invalid/api/sso/callback".into()],
            &[],
            false
        )
        .is_none()
    );
}
