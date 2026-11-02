use webdock_export_protocol::{SourceKind, SourceRef};
#[test]
fn source_references_cannot_select_urls_sql_files_or_mutable_repository_heads() {
    let base = SourceRef {
        version: 1,
        kind: SourceKind::Project,
        id: "123".into(),
        binding: "a".repeat(64),
        commit: Some("b".repeat(40)),
    };
    assert!(base.validate().is_ok());
    for id in [
        "../../private",
        "https://secret@db/",
        "SELECT 1",
        "",
        "0123",
    ] {
        let mut v = base.clone();
        v.id = id.into();
        assert!(v.validate().is_err());
    }
    for commit in [Some("main".into()), Some("b".repeat(39))] {
        let mut v = base.clone();
        v.commit = commit;
        assert!(v.validate().is_err());
    }
    let mut pending = base.clone();
    pending.commit = None;
    assert!(pending.validate().is_ok());
    assert!(pending.finished().is_err());
    let mut v = base.clone();
    v.kind = SourceKind::Postgres;
    assert!(v.validate().is_err());
    v.commit = None;
    assert!(v.validate().is_ok());
}
#[test]
fn readonly_credentials_are_domain_separated_and_database_bound() {
    let original = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8";
    assert_eq!(
        webdock_export_protocol::readonly_password("123", original).unwrap(),
        "xu3QJWlQItYM8d2EQKym15VPXVxXhI9ildzh21E0faI"
    );
    assert_ne!(
        webdock_export_protocol::readonly_password("124", original).unwrap(),
        webdock_export_protocol::readonly_password("123", original).unwrap()
    );
    assert!(webdock_export_protocol::readonly_password("123", "imported-password").is_err());
    assert!(webdock_export_protocol::readonly_password("123';DROP ROLE", original).is_err());
}

#[test]
fn completed_buildlog_is_a_distinct_id_bound_binary_source_without_git_commit() {
    let value = serde_json::json!({"version":1,"kind":"buildlog","id":"123","binding":"a".repeat(64),"commit":null});
    let source =
        serde_json::from_value::<SourceRef>(value).expect("completed buildlog source kind");
    assert!(source.finished().is_ok());
    let mut forged = serde_json::to_value(source).unwrap();
    forged["commit"] = serde_json::json!("b".repeat(40));
    assert!(
        serde_json::from_value::<SourceRef>(forged)
            .unwrap()
            .validate()
            .is_err()
    );
}
