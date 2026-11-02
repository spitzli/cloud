//! Actual owned Native database export qualification. Only public refs/counts/hashes leave memory.
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};
use std::{
    io::{Read, Seek},
    path::Path,
    process::Stdio,
    time::Duration,
};
use webdock_databases::DatabaseService;
use webdock_identity::{Principal, RlsSigner};
fn tool(name: &str) -> String {
    assert!(["psql", "pg_dump"].contains(&name));
    [
        format!("/usr/lib/postgresql/18/bin/{name}"),
        format!("/usr/bin/{name}"),
        format!("/usr/local/bin/{name}"),
    ]
    .into_iter()
    .find(|p| Path::new(p).is_file())
    .expect("owned PG18 client required")
}
pub(super) async fn qualify(
    service: &DatabaseService,
    principal: &Principal,
    business: &DatabaseConnection,
    customer: &str,
    project: &str,
    database: &str,
) -> Value {
    let signer = RlsSigner::new(
        std::env::var("DATABASE_RLS_SECRET").unwrap(),
        "webdock_auth_runtime",
    )
    .unwrap();
    let tx = signer.begin(business, principal).await.unwrap();
    let actor = webdock_databases::DatabaseExportSubject {
        customer,
        project,
        subject: &principal.user_id,
        session: &principal.session_id,
    };
    let source = service
        .export_reference_in(&tx, &actor, database)
        .await
        .expect("actual dedicated source reference");
    let material = service
        .export_material_in(&tx, &actor, &source, chrono::Utc::now().timestamp() + 35)
        .await
        .expect("actual readonly material");
    assert_eq!(material.username, format!("wd_export_{database}"));
    assert_eq!(material.database, format!("db_{database}"));
    assert!(!material.ca_certificate.is_empty());
    tx.commit().await.unwrap();
    let ca = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(ca.path(), material.ca_certificate.as_bytes()).unwrap();
    let configured = |binary: &str| {
        let mut c = tokio::process::Command::new(tool(binary));
        c.env_clear()
            .env("PGHOST", &material.host)
            .env("PGPORT", material.port.to_string())
            .env("PGUSER", &material.username)
            .env("PGPASSWORD", &material.password)
            .env("PGDATABASE", &material.database)
            .env("PGSSLMODE", "verify-full")
            .env("PGSSLROOTCERT", ca.path())
            .env("PGCONNECT_TIMEOUT", "3")
            .env(
                "PGOPTIONS",
                "-c default_transaction_read_only=on -c statement_timeout=20000",
            )
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        c
    };
    let query = |sql: &str| {
        let mut c = configured("psql");
        c.args([
            "--no-password",
            "--no-psqlrc",
            "-X",
            "-A",
            "-t",
            "-v",
            "ON_ERROR_STOP=1",
            "-c",
            sql,
        ]);
        c
    };
    let result = tokio::time::timeout(
        Duration::from_secs(8),
        query("SELECT count(*) FROM app.review_probe WHERE id=1").output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        result.status.success(),
        "actual readonly TLS connection failed; private diagnostics withheld"
    );
    assert_eq!(String::from_utf8(result.stdout).unwrap().trim(), "1");
    let flags=tokio::time::timeout(Duration::from_secs(8),query("SELECT NOT(rolsuper OR rolcreaterole OR rolcreatedb OR rolreplication OR rolbypassrls OR rolinherit) AND NOT EXISTS(SELECT 1 FROM pg_auth_members WHERE member=r.oid) FROM pg_roles r WHERE rolname=current_user").output()).await.unwrap().unwrap();
    assert!(flags.status.success());
    assert_eq!(flags.stdout, b"t\n");
    for sql in [
        "INSERT INTO app.review_probe(id) VALUES(999999)",
        "CREATE TABLE app.export_forbidden(value integer)",
    ] {
        let mut command = configured("psql");
        command
            .args([
                "--no-password",
                "--no-psqlrc",
                "-X",
                "-A",
                "-t",
                "-q",
                "-v",
                "ON_ERROR_STOP=1",
                "-v",
                "VERBOSITY=sqlstate",
                "-c",
                "SET default_transaction_read_only=off",
                "-c",
                "SHOW transaction_read_only",
                "-c",
                sql,
            ])
            .stderr(Stdio::piped());
        let result = tokio::time::timeout(Duration::from_secs(8), command.output())
            .await
            .unwrap()
            .unwrap();
        assert!(
            !result.status.success(),
            "readonly source unexpectedly writes"
        );
        assert_eq!(
            result.stdout, b"off\n",
            "ACL denial must be independent of transaction readonly"
        );
        assert!(
            result.stderr.windows(5).any(|w| w == b"42501"),
            "expected insufficient_privilege, private diagnostics withheld"
        );
    }
    let version = tokio::process::Command::new(tool("pg_dump"))
        .arg("--version")
        .output()
        .await
        .unwrap();
    assert!(
        String::from_utf8(version.stdout)
            .unwrap()
            .starts_with("pg_dump (PostgreSQL) 18.")
    );
    let mut dump = tempfile::tempfile().unwrap();
    let mut command = configured("pg_dump");
    command
        .args([
            "--format=custom",
            "--compress=gzip:6",
            "--schema=app",
            "--no-owner",
            "--no-privileges",
            "--no-password",
            "--lock-wait-timeout=5s",
        ])
        .stdout(Stdio::from(dump.try_clone().unwrap()));
    let status = tokio::time::timeout(Duration::from_secs(25), command.status())
        .await
        .unwrap()
        .unwrap();
    assert!(
        status.success(),
        "actual Native source export failed; private diagnostics withheld"
    );
    let size = dump.metadata().unwrap().len();
    assert!(size > 100 && size <= 32 * 1024 * 1024);
    dump.rewind().unwrap();
    let mut magic = [0; 5];
    dump.read_exact(&mut magic).unwrap();
    assert_eq!(&magic, b"PGDMP");
    dump.rewind().unwrap();
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let n = dump.read(&mut buffer).unwrap();
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    let tx = signer.begin(business, principal).await.unwrap();
    assert_eq!(
        service
            .export_reference_in(&tx, &actor, database)
            .await
            .unwrap(),
        source
    );
    tx.commit().await.unwrap();
    let safe = json!({"fixtureOnly":true,"source":source,"actualManagedSource":true,"readonlyTLS":true,"noInheritedOrWriteAuthority":true,"DMLAndDDLDenied":true,"actualProbeRows":1,"compressedBytes":size,"sha256":hex::encode(hash.finalize()),"materialPersisted":false});
    assert!(!safe.to_string().contains(&material.password));
    assert!(!safe.to_string().contains(&material.host));
    safe
}

#[expect(
    clippy::too_many_arguments,
    reason = "Exact actor, tenant, project, database and prior source lineage remain explicit in this qualification boundary."
)]
pub(super) async fn reject_transition(
    service: &DatabaseService,
    principal: &Principal,
    business: &DatabaseConnection,
    customer: &str,
    project: &str,
    database: &str,
    previous: &Value,
    stage: &str,
) -> Value {
    assert!(["rotation", "suspension"].contains(&stage));
    assert_eq!(previous["source"]["id"], database);
    assert_eq!(previous["source"]["kind"], "postgres");
    let signer = RlsSigner::new(
        std::env::var("DATABASE_RLS_SECRET").unwrap(),
        "webdock_auth_runtime",
    )
    .unwrap();
    let tx = signer.begin(business, principal).await.unwrap();
    let actor = webdock_databases::DatabaseExportSubject {
        customer,
        project,
        subject: &principal.user_id,
        session: &principal.session_id,
    };
    let reference = serde_json::from_value(previous["source"].clone())
        .expect("prior sanitized source reference");
    assert!(
        service
            .export_material_in(&tx, &actor, &reference, chrono::Utc::now().timestamp() + 35)
            .await
            .is_err(),
        "stale source must not yield material after actual lifecycle transition"
    );
    if stage == "suspension" {
        assert!(
            service
                .export_reference_in(&tx, &actor, database)
                .await
                .is_err(),
            "paused Native source must refuse new references"
        );
        tx.commit().await.unwrap();
        return json!({"fixtureOnly":true,"stage":"suspension","source":reference,"oldReferenceDenied":true,"newReferenceDenied":true,"materialPersisted":false});
    }
    let fresh = service
        .export_reference_in(&tx, &actor, database)
        .await
        .expect("actual rotated source ready");
    assert_ne!(
        fresh.binding, reference.binding,
        "actual rotation must advance source identity"
    );
    tx.commit().await.unwrap();
    let mut result = qualify(service, principal, business, customer, project, database).await;
    assert_eq!(result["source"], serde_json::to_value(&fresh).unwrap());
    result["stage"] = json!("rotation");
    result["oldReferenceDenied"] = json!(true);
    result
}
