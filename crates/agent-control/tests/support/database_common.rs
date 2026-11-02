use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use std::sync::Arc;
use webdock_identity::{CookiePolicy, IdentityService, PasswordAuthentication, SecretKeys};
pub async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .expect("owned fixture SQL");
}
pub async fn row(db: &DatabaseConnection, sql: &str, id: &str) -> serde_json::Value {
    db.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        [id.into()],
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get("", "value")
    .unwrap()
}
pub fn url(key: &str, name: &str) -> String {
    let mut url = url::Url::parse(&std::env::var(key).unwrap()).unwrap();
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    assert_eq!(url.path(), "/webdock_rls_test");
    assert!(url.query().is_none());
    assert!(
        name.starts_with("webdock_native_db_")
            && name.ends_with("_test")
            && name
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
    );
    url.set_path(&format!("/{name}"));
    url.to_string()
}
pub async fn identity(
    auth: DatabaseConnection,
    ids: Arc<webdock_persistence::IdAllocator>,
) -> (Arc<IdentityService>, webdock_identity::Principal) {
    let secret = std::env::var("BETTER_AUTH_SECRET").unwrap();
    let identity = Arc::new(
        IdentityService::new(
            auth.clone(),
            CookiePolicy::new(false, secret.clone()).unwrap(),
        )
        .with_secret_keys(
            SecretKeys::new(
                std::collections::BTreeMap::from([(0, secret.clone())]),
                0,
                Some(secret),
            )
            .unwrap(),
        )
        .with_allocator(ids.clone()),
    );
    let email = format!(
        "native-db-{}@example.invalid",
        ids.allocate().await.unwrap()
    );
    let id = identity
        .create_local_account(
            &email,
            "Owned database operator",
            "owned-fixture-password-before",
            true,
        )
        .await
        .unwrap();
    exec(
        &auth,
        "UPDATE webdock_auth.\"user\" SET \"emailVerified\"=true WHERE id=$1",
        vec![id.into()],
    )
    .await;
    let PasswordAuthentication::SetupRequired {
        session: Some(setup),
        ..
    } = identity
        .authenticate_password(&email, "owned-fixture-password-before")
        .await
        .unwrap()
    else {
        panic!("operator setup required")
    };
    let changed = identity
        .change_password(
            setup.set_cookie.split(';').next().unwrap(),
            "owned-fixture-password-before",
            "owned-fixture-password-after",
        )
        .await
        .unwrap();
    let cookie = changed.set_cookie.split(';').next().unwrap();
    let enrollment = identity
        .begin_totp_enrollment(cookie, "owned-fixture-password-after")
        .await
        .unwrap();
    let totp = totp_rs::Totp::from_url(&enrollment.totp_uri).unwrap();
    let completed = identity
        .confirm_totp_enrollment(cookie, &totp.generate_current().to_string())
        .await
        .unwrap();
    let actor = identity
        .authenticate_cookie(completed.set_cookie.split(';').next().unwrap())
        .await
        .unwrap()
        .unwrap();
    (identity, actor)
}
pub async fn connection(name: &str, key: &str) -> DatabaseConnection {
    Database::connect(url(key, name))
        .await
        .expect("owned fixture role connection")
}
