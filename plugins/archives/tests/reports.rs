//! Real Core RLS projection: user-editable spreadsheet formulas never become report cells.
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};
use serde_json::{Value, json};
use std::sync::Arc;
use webdock_identity::{CookiePolicy, IdentityService};
use webdock_plugin_host::{
    AdmissionInput, ExportSources, FileBindings, HostService, InvocationScope,
};
use webdock_plugin_runtime::PeerIdentity;
#[allow(
    dead_code,
    reason = "existing owned Core fixture also serves job tests"
)]
#[path = "../../../crates/plugin-host/tests/support/jobs_fixture.rs"]
mod fixture;
#[path = "../../../crates/control-plane/src/plugin_exports.rs"]
mod source;
fn peer() -> PeerIdentity {
    PeerIdentity::Unix {
        uid: rustix::process::geteuid().as_raw(),
    }
}
async fn sql(db: &impl ConnectionTrait, text: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        text,
        values,
    ))
    .await
    .unwrap();
}
#[tokio::test]
#[ignore = "owns a fresh disposable Core PostgreSQL; never uses retained customer services"]
async fn observed_report_excludes_formula_control_names_and_foreign_project_resources() {
    let f = fixture::JobFixture::start_with_connections(8).await;
    let business = f.database.role("webdock_auth_runtime").await;
    let service = Arc::new(
        webdock_databases::DatabaseService::new(
            business.clone(),
            f.auth.clone(),
            f.database.role("webdock_worker_runtime").await,
            f.identity.clone(),
            None,
            Arc::new(
                webdock_persistence::IdAllocator::connect(
                    f.database.url.as_str(),
                    webdock_ids::Identity::platform(0, 4).unwrap(),
                )
                .await
                .unwrap(),
            ),
            "owned-plugin-jobs-rls-secret-32bytes".into(),
            "owned-native-export-cipher-secret-32bytes",
            "http://127.0.0.1:1",
        )
        .await
        .unwrap(),
    );
    let source = source::Sources {
        databases: service,
        repository: None,
    };
    let tx = f.auth.begin().await.unwrap();
    let actor = f
        .identity
        .authenticate_session_in(&tx, "member", "member-session")
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    let signer = webdock_identity::RlsSigner::new(
        "owned-plugin-jobs-rls-secret-32bytes",
        "webdock_auth_runtime",
    )
    .unwrap();
    let scope = InvocationScope {
        plugin_id: "archives".into(),
        customer_id: "123".into(),
        project_id: "124".into(),
        operation_id: "report-test".into(),
        subject: "member".into(),
        session_id: "member-session".into(),
        generation: 1,
        grant_revision: 0,
    };
    sql(&f.owner,"INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES('999','Foreign owned source','123')",vec![]).await;
    sql(&f.owner,"INSERT INTO webdock_auth.plugin_resource(plugin_id,resource_id,offering_id,customer_id,project_id,name,created_via,created_by) VALUES('job-fixture','foreign-only','unit','123','999','=FOREIGN_FORMULA','operator','operator')",vec![]).await;
    for value in [
        "=HYPERLINK(\"https://example.invalid\",\"x\")",
        "+SUM(1,1)",
        "-1+1",
        "@SUM(A1:A2)",
        "\t=1+1",
        "\u{1}=1+1",
    ] {
        sql(
            &f.owner,
            "UPDATE webdock_admin.projects SET name=$1 WHERE id='124'",
            vec![value.into()],
        )
        .await;
        sql(
            &f.owner,
            "UPDATE webdock_auth.plugin_resource SET name=$1 WHERE resource_id='resource-one'",
            vec![value.into()],
        )
        .await;
        let tx = signer.begin(&business, &actor).await.unwrap();
        let reference = source
            .bind(
                &scope,
                &tx,
                webdock_export_protocol::SourceKind::Report,
                "124",
            )
            .await
            .unwrap();
        let bytes = source.report(&scope, &tx, &reference).await.unwrap();
        let csv = String::from_utf8(bytes).unwrap();
        assert!(csv.contains("resources,job-fixture.unit,1\n"));
        assert!(!csv.contains(value));
        assert!(!csv.contains("FOREIGN_FORMULA"));
        for cell in csv.lines().flat_map(|line| line.split(',')) {
            assert!(!cell.trim_start().starts_with(['=', '+', '-', '@']));
            assert!(!cell.chars().any(char::is_control));
        }
        tx.commit().await.unwrap();
    }
    sql(
        &f.owner,
        "DELETE FROM webdock_auth.member WHERE id='member-binding'",
        vec![],
    )
    .await;
    let tx = signer.begin(&business, &actor).await.unwrap();
    assert!(
        source
            .bind(
                &scope,
                &tx,
                webdock_export_protocol::SourceKind::Report,
                "124"
            )
            .await
            .is_err()
    );
}
