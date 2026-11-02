//! Owned local browser fixture. Never use a retained customer database.
use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};
use std::{collections::BTreeMap, sync::Arc};
use webdock_identity::{CookiePolicy, IdentityService, PasswordAuthentication, SecretKeys};

fn local(key: &str) -> String {
    let raw = std::env::var(key).expect("explicit test fixture");
    let url = url::Url::parse(&raw).unwrap();
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    assert_eq!(url.path(), "/webdock_rls_test");
    assert!(url.query().is_none());
    raw
}
#[tokio::main]
async fn main() {
    let auth_url = local("AUTH_DATABASE_URL");
    let auth = Database::connect(&auth_url).await.unwrap();
    let owner = Database::connect(local("WEBDOCK_TEST_MIGRATION_DATABASE_URL"))
        .await
        .unwrap();
    let secret = std::env::var("BETTER_AUTH_SECRET").unwrap();
    let allocator = Arc::new(
        webdock_persistence::IdAllocator::connect(
            &auth_url,
            webdock_ids::Identity::platform(0, 2).unwrap(),
        )
        .await
        .unwrap(),
    );
    let identity = IdentityService::new(auth, CookiePolicy::new(false, &secret).unwrap())
        .with_secret_keys(
            SecretKeys::new(BTreeMap::from([(1, secret.clone())]), 1, Some(secret)).unwrap(),
        )
        .with_allocator(allocator.clone());
    let email = format!("browser-{}@example.invalid", uuid::Uuid::new_v4());
    let password = uuid::Uuid::new_v4().to_string();
    let replacement = uuid::Uuid::new_v4().to_string();
    let user = identity
        .create_local_account(&email, "Local browser operator", &password, true)
        .await
        .unwrap();
    owner
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_auth.\"user\" SET \"emailVerified\"=true WHERE id=$1",
            [user.clone().into()],
        ))
        .await
        .unwrap();
    let PasswordAuthentication::SetupRequired {
        session: Some(setup),
        ..
    } = identity
        .authenticate_password(&email, &password)
        .await
        .unwrap()
    else {
        panic!("setup required");
    };
    let changed = identity
        .change_password(
            setup.set_cookie.split(';').next().unwrap(),
            &password,
            &replacement,
        )
        .await
        .unwrap();
    let cookie = changed.set_cookie.split(';').next().unwrap();
    let enrollment = identity
        .begin_totp_enrollment(cookie, &replacement)
        .await
        .unwrap();
    let totp = totp_rs::Totp::from_url(&enrollment.totp_uri).unwrap();
    let session = identity
        .confirm_totp_enrollment(cookie, &totp.generate_current().to_string())
        .await
        .unwrap();
    owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES($1,'Local browser operator',$2,'operator',$3)",[allocator.allocate().await.unwrap().to_string().into(),email.clone().into(),user.clone().into()])).await.unwrap();
    let value = serde_json::json!({"email":email,"userID":user,"password":replacement,"cookie":session.set_cookie.split(';').next().unwrap()});
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut output = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(".runtime-test/browser.json")
            .unwrap();
        output.write_all(value.to_string().as_bytes()).unwrap();
    }
    println!("Local browser fixture prepared; credentials remain in an ignored private file.");
}
