//! Explicit owned local fixture; credentials stay in a private ignored file.
use clap::Parser;
use sea_orm::{ConnectionTrait, Database, DbBackend, Statement, TransactionTrait};
use std::{collections::BTreeMap, sync::Arc};
use webdock_identity::{CookiePolicy, IdentityService, SecretKeys};
use webdock_oauth::*;
#[derive(Parser)]
struct Args {
    /// Restore only missing client/projection rows for an exact recorded local fixture.
    #[arg(long)]
    repair: bool,
}
fn fixture_path() -> std::path::PathBuf {
    let name =
        std::env::var("WEBDOCK_ACCOUNT_FIXTURE").unwrap_or_else(|_| "account-browser.json".into());
    assert!(
        name.len() <= 96
            && name.starts_with("account-browser")
            && name.ends_with(".json")
            && !name.contains("..")
            && name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    );
    std::path::Path::new(".runtime-test").join(name)
}
fn read_fixture(path: &std::path::Path) -> Option<Vec<u8>> {
    match webdock_control_plane::config::read_private_file(
        &std::env::current_dir().unwrap().join(path),
    ) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => panic!("Owned private account fixture required"),
    }
}
fn guarded(key: &str) -> String {
    let raw = std::env::var(key).unwrap();
    let u = url::Url::parse(&raw).unwrap();
    assert!(["postgres", "postgresql"].contains(&u.scheme()));
    assert_eq!(u.host_str(), Some("127.0.0.1"));
    assert_eq!(u.path(), "/webdock_rls_test");
    assert!(u.query().is_none() && u.fragment().is_none());
    raw
}
#[tokio::main]
async fn main() {
    let args = Args::parse();
    let path = fixture_path();
    let url = guarded("AUTH_DATABASE_URL");
    let db = Database::connect(&url).await.unwrap();
    let owner = Database::connect(guarded("WEBDOCK_TEST_MIGRATION_DATABASE_URL"))
        .await
        .unwrap();
    if args.repair {
        repair(&owner, &url, &path).await;
        return;
    }
    if let Some(bytes) = read_fixture(&path) {
        let cleanup = owner.begin().await.unwrap();
        let old: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let user = old["userID"].as_str().unwrap();
        let client = old["clientID"].as_str().unwrap();
        let owned=cleanup.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM webdock_auth.\"user\" WHERE id=$1 AND email=$2 AND name='Account browser fixture' FOR UPDATE",[user.into(),old["email"].as_str().unwrap().into()])).await.unwrap().is_some();
        assert!(
            owned,
            "Only the exact previously created fixture may be replaced"
        );
        for sql in [
            "DELETE FROM webdock_auth.identity_browser_flow WHERE client_id=$1",
            "DELETE FROM webdock_auth.app_binding WHERE client_id=$1",
            "DELETE FROM webdock_auth.\"oauthClient\" WHERE \"clientId\"=$1",
        ] {
            cleanup
                .execute_raw(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    sql,
                    [client.into()],
                ))
                .await
                .unwrap();
        }
        cleanup
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM webdock_admin.users WHERE auth_subject=$1",
                [user.into()],
            ))
            .await
            .unwrap();
        cleanup
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM webdock_auth.\"user\" WHERE id=$1",
                [user.into()],
            ))
            .await
            .unwrap();
        cleanup
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM webdock_auth.jwks WHERE id=$1",
                [old["signingID"].as_str().unwrap().into()],
            ))
            .await
            .unwrap();
        cleanup.commit().await.unwrap();
        std::fs::remove_file(&path).unwrap();
    }
    let secret = std::env::var("BETTER_AUTH_SECRET").unwrap();
    let allocator = Arc::new(
        webdock_persistence::IdAllocator::connect(
            &url,
            webdock_ids::Identity::platform(0, 2).unwrap(),
        )
        .await
        .unwrap(),
    );
    let identity = Arc::new(
        IdentityService::new(db.clone(), CookiePolicy::new(false, &secret).unwrap())
            .with_allocator(allocator.clone()),
    );
    let keys = Arc::new(
        SecretKeys::new(
            BTreeMap::from([(1, secret.clone())]),
            1,
            Some(secret.clone()),
        )
        .unwrap(),
    );
    let oauth = OAuthService::new(
        db.clone(),
        identity.clone(),
        allocator.clone(),
        keys,
        OAuthConfig::new(
            "http://127.0.0.1:3125/api/auth",
            "http://127.0.0.1:3120/api/mcp",
            None,
        )
        .unwrap(),
    );
    let signing = oauth.provision_signing_key(&owner).await.unwrap();
    let client = oauth
        .provision_client(
            &owner,
            ClientRegistration {
                name: "Webdock Console".into(),
                redirect_uris: vec![
                    "http://127.0.0.1:3121/api/sso/callback".into(),
                    "http://127.0.0.1:3120/api/sso/callback".into(),
                ],
                post_logout_redirect_uris: vec![
                    "http://127.0.0.1:3121/login".into(),
                    "http://127.0.0.1:3120/login".into(),
                ],
                scopes: vec!["openid".into(), "profile".into(), "email".into()],
                auth_method: ClientAuthMethod::ClientSecretPost,
                allow_refresh: true,
                skip_consent: false,
                purpose: ClientPurpose::Portal,
            },
        )
        .await
        .unwrap();
    let email = format!(
        "account-ui-{}@example.invalid",
        allocator.allocate().await.unwrap()
    );
    let password = uuid::Uuid::new_v4().to_string();
    let replacement = uuid::Uuid::new_v4().to_string();
    let user = identity
        .create_local_account(&email, "Account browser fixture", &password, true)
        .await
        .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE webdock_auth.\"user\" SET \"emailVerified\"=true WHERE id=$1",
        [user.clone().into()],
    ))
    .await
    .unwrap();
    owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES($1,'Account browser fixture',$2,'operator',$3)",[allocator.allocate().await.unwrap().to_string().into(),email.clone().into(),user.clone().into()])).await.unwrap();
    let value = serde_json::json!({"userID":user,"email":email,"password":password,"replacement":replacement,"clientID":client.client_id,"clientSecret":client.client_secret,"bindingID":client.binding_id,"cookieSecret":uuid::Uuid::new_v4().to_string(),"signingID":signing});
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    output.write_all(value.to_string().as_bytes()).unwrap();
    println!("Owned account browser fixture prepared; credentials remain private.");
}

async fn repair(owner: &sea_orm::DatabaseConnection, auth_url: &str, path: &std::path::Path) {
    let old: serde_json::Value =
        serde_json::from_slice(&read_fixture(path).expect("Recorded fixture required")).unwrap();
    let user = old["userID"].as_str().unwrap();
    let client = old["clientID"].as_str().unwrap();
    let binding = old["bindingID"].as_str().unwrap();
    let email = old["email"].as_str().unwrap();
    let ids = webdock_persistence::IdAllocator::connect(
        auth_url,
        webdock_ids::Identity::platform(0, 2).unwrap(),
    )
    .await
    .unwrap();
    let portal_id = ids.allocate().await.unwrap().to_string();
    let client_row_id = ids.allocate().await.unwrap().to_string();
    let tx = owner.begin().await.unwrap();
    let owned = tx.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM webdock_auth.\"user\"
         WHERE id=$1 AND email=$2 AND name='Account browser fixture' AND role='operator' FOR UPDATE",
        [user.into(), email.into()],
    )).await.unwrap();
    assert!(
        owned.is_some(),
        "Only the recorded owned fixture may be repaired"
    );
    for (sql, values) in [
        ("INSERT INTO webdock_admin.users(id,name,email,role,auth_subject)
          SELECT $1,'Account browser fixture',$2,'operator',$3
          WHERE NOT EXISTS(SELECT 1 FROM webdock_admin.users WHERE auth_subject=$3)",
         vec![portal_id.into(), email.into(), user.into()]),
        ("INSERT INTO webdock_auth.\"oauthClient\"
          (id,\"clientId\",\"clientSecret\",name,disabled,\"skipConsent\",\"enableEndSession\",scopes,
          \"redirectUris\",\"postLogoutRedirectUris\",\"grantTypes\",\"responseTypes\",
          \"tokenEndpointAuthMethod\",\"requirePKCE\",\"subjectType\",metadata,\"createdAt\",\"updatedAt\")
          SELECT $1,$2,$3,'Webdock Console',false,false,true,'[\"openid\",\"profile\",\"email\"]'::jsonb,
          '[\"http://127.0.0.1:3121/api/sso/callback\",\"http://127.0.0.1:3120/api/sso/callback\"]'::jsonb,
          '[\"http://127.0.0.1:3121/login\",\"http://127.0.0.1:3120/login\"]'::jsonb,
          '[\"authorization_code\",\"refresh_token\"]'::jsonb,'[\"code\"]'::jsonb,
          'client_secret_post',true,'public',jsonb_build_object('webdock_binding',$4::text),now(),now()
          WHERE NOT EXISTS(SELECT 1 FROM webdock_auth.\"oauthClient\" WHERE \"clientId\"=$2)",
         vec![client_row_id.into(),client.into(),legacy_token_hash(old["clientSecret"].as_str().unwrap()).into(),binding.into()]),
        ("INSERT INTO webdock_auth.app_binding(id,client_id,label,enabled)
          SELECT $1,$2,'Webdock Console',true
          WHERE NOT EXISTS(SELECT 1 FROM webdock_auth.app_binding WHERE id=$1)",
         vec![binding.into(),client.into()]),
    ] {
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,sql,values)).await.unwrap();
    }
    let valid = tx
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT c.\"clientSecret\"=$4 AND c.metadata->>'webdock_binding'=$3
          AND p.email=$5 AND p.name='Account browser fixture' AND p.role='operator'
          AND c.name='Webdock Console'
          AND EXISTS(SELECT 1 FROM webdock_auth.jwks WHERE id=$6) AS valid
         FROM webdock_auth.\"oauthClient\" c
         JOIN webdock_auth.app_binding b ON b.client_id=c.\"clientId\" AND b.id=$3
         JOIN webdock_admin.users p ON p.auth_subject=$1
         WHERE c.\"clientId\"=$2",
            vec![
                user.into(),
                client.into(),
                binding.into(),
                legacy_token_hash(old["clientSecret"].as_str().unwrap()).into(),
                email.into(),
                old["signingID"].as_str().unwrap().into(),
            ],
        ))
        .await
        .unwrap()
        .unwrap();
    assert!(
        valid.try_get::<bool>("", "valid").unwrap(),
        "Fixture references changed"
    );
    tx.commit().await.unwrap();
    println!("Owned account fixture references restored; identity and grants retained.");
}
