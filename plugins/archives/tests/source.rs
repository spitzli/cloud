use webdock_archives::model::{Archive, Kind, State};
use webdock_export_protocol::{SourceKind, SourceRef};
#[test]
fn every_observed_producer_requires_its_exact_managed_source_kind_and_binding() {
    let mut archive = Archive {
        id: uuid::Uuid::new_v4(),
        customer: "123".into(),
        project: "124".into(),
        kind: Kind::Postgres,
        state: State::Pending,
        epoch: uuid::Uuid::new_v4().to_string(),
        expires_at: 2000,
        source_binding: Some("a".repeat(64)),
        core_source: None,
        job: None,
        failure: None,
        receipt: None,
        diagnostics: None,
    };
    for kind in [Kind::Postgres, Kind::Project, Kind::Report] {
        archive.kind = kind;
        assert!(archive.source().is_err());
        archive.core_source = Some(SourceRef {
            version: 1,
            kind: SourceKind::Postgres,
            id: "125".into(),
            binding: "a".repeat(64),
            commit: None,
        });
        assert_eq!(archive.source().is_ok(), kind == Kind::Postgres);
        archive.source_binding = Some("b".repeat(64));
        assert!(archive.source().is_err());
        archive.source_binding = Some("a".repeat(64));
        archive.core_source = None;
    }
    archive.kind = Kind::Diagnostics;
    assert!(archive.source().unwrap().is_none());
}

#[test]
fn completed_buildlog_receipt_is_not_diagnostics_and_cannot_exceed_64k() {
    let mut archive:Archive=serde_json::from_value(serde_json::json!({"id":"11111111-1111-4111-8111-111111111111","customer":"123","project":"124","kind":"buildlog","state":"ready","epoch":"22222222-2222-4222-8222-222222222222","expires_at":2000,"source_binding":"a".repeat(64),"core_source":{"version":1,"kind":"buildlog","id":"125","binding":"a".repeat(64),"commit":null},"job":"job-actual","failure":null,"receipt":{"backendId":"owned","binding":"a".repeat(64),"objectKey":"archives/123/124/11111111-1111-4111-8111-111111111111/22222222-2222-4222-8222-222222222222","sizeBytes":65536,"sha256":"b".repeat(64)},"diagnostics":null})).expect("buildlog receipt kind");
    assert!(archive.ready(1000).is_ok());
    archive.receipt.as_mut().unwrap().size_bytes = 65537;
    assert!(
        archive.ready(1000).is_err(),
        "receipt may not relax buildlog output limit"
    );
    archive.receipt.as_mut().unwrap().size_bytes = 100;
    archive.core_source = None;
    assert!(
        archive.ready(1000).is_err(),
        "buildlog always needs live Core source binding"
    );
}
