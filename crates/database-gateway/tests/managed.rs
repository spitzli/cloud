use std::{path::PathBuf, time::Duration};
use webdock_database_gateway::{
    control::ManagedAccess,
    managed::{ManagedRuntimes, ProcessConfig, ProcessManager},
};
fn access() -> ManagedAccess {
    serde_json::from_value(serde_json::json!({"kind":"managed","scopeID":"123","subject":"fixture","connectionID":"fixture-connection","profile":"schema","revision":"1","expiresAt":(chrono::Utc::now()+chrono::Duration::minutes(1)).to_rfc3339(),"database":{"host":"127.0.0.1","port":1,"database":"fixture","username":"fixture","password":"synthetic-unused-fixture-password","caCertificate":"synthetic-unused-fixture-ca"}})).unwrap()
}
#[tokio::test]
#[ignore = "requires explicit existing local database-browser binary and public web root"]
async fn actual_runtime_is_shared_only_with_same_identity_and_reaped_on_release() {
    let executable = PathBuf::from(std::env::var("WEBDOCK_MANAGED_RUNTIME_TEST_BINARY").unwrap());
    let web_root = PathBuf::from(std::env::var("WEBDOCK_MANAGED_RUNTIME_TEST_WEB_ROOT").unwrap());
    assert!(executable.is_absolute() && web_root.is_absolute());
    let temporary = tempfile::tempdir().unwrap();
    let manager = ProcessManager::new(ProcessConfig {
        executable,
        web_root: Some(web_root),
        temporary_directory: temporary.path().into(),
        capacity: 1,
        startup_timeout: Duration::from_secs(10),
    })
    .unwrap();
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let manager = manager.clone();
        tasks.push(tokio::spawn(async move {
            manager.acquire("fixture-session", &access()).await.unwrap()
        }));
    }
    let runtime = tasks.remove(0).await.unwrap();
    for task in tasks {
        let other = task.await.unwrap();
        assert!(runtime.access.runtime_origin == other.access.runtime_origin);
        assert!(runtime.access.proxy_secret == other.access.proxy_secret);
    }
    assert_eq!(std::fs::read_dir(temporary.path()).unwrap().count(), 1);
    assert!(manager.acquire("another-session", &access()).await.is_err());
    let mut changed = access();
    changed.revision = "2".into();
    assert!(manager.acquire("fixture-session", &changed).await.is_err());
    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/session", runtime.access.runtime_origin))
        .header("Origin", &runtime.authentication_origin)
        .header("X-Tabularis-User", "fixture")
        .header("X-Tabularis-Proxy-Secret", &runtime.access.proxy_secret)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let session: serde_json::Value = response.json().await.unwrap();
    assert_eq!(session["authenticated"], true);
    assert_eq!(session["apiVersion"], "v1");
    manager.release("fixture-session").await;
    assert_eq!(std::fs::read_dir(temporary.path()).unwrap().count(), 0);
    assert!(
        reqwest::get(format!("{}/healthz", runtime.access.runtime_origin))
            .await
            .is_err()
    );
    // Dropping a waiting request must not launch a second process for the same session.
    let pending = manager.clone();
    let first = tokio::spawn(async move { pending.acquire("cancelled-waiter", &access()).await });
    for _ in 0..100 {
        if std::fs::read_dir(temporary.path()).unwrap().count() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    first.abort();
    manager
        .acquire("cancelled-waiter", &access())
        .await
        .unwrap();
    assert_eq!(std::fs::read_dir(temporary.path()).unwrap().count(), 1);
    manager.shutdown().await;
    assert_eq!(std::fs::read_dir(temporary.path()).unwrap().count(), 0);
    assert!(manager.acquire("after-shutdown", &access()).await.is_err());
}
