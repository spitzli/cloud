use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};
use std::{collections::BTreeMap, sync::Arc};
use webdock_identity::{CookiePolicy, IdentityService, PasswordAuthentication, SecretKeys};
use webdock_oauth::*;
fn url(key: &str) -> String {
    let raw = std::env::var(key).unwrap();
    let u = url::Url::parse(&raw).unwrap();
    assert!([Some("localhost"), Some("127.0.0.1"), Some("[::1]")].contains(&u.host_str()));
    assert!(["postgres", "postgresql"].contains(&u.scheme()));
    assert_eq!(u.path(), "/webdock_rls_test");
    assert!(u.query().is_none() && u.fragment().is_none());
    raw
}
fn cookie(h: &str) -> &str {
    h.split(';').next().unwrap()
}
#[tokio::test]
#[ignore = "requires owned retained-schema restricted fixture"]
async fn public_client_grants_follow_live_tenant_membership_and_explicit_revocation() {
    let auth_url = url("AUTH_DATABASE_URL");
    let db = Database::connect(auth_url.clone()).await.unwrap();
    let offline = Database::connect(url("WEBDOCK_TEST_MIGRATION_DATABASE_URL"))
        .await
        .unwrap();
    offline
        .execute_unprepared(include_str!("../migrations/0001_oauth_authorization.sql"))
        .await
        .unwrap();
    let allocator = Arc::new(
        webdock_persistence::IdAllocator::connect(
            &auth_url,
            webdock_ids::Identity::platform(0, 2).unwrap(),
        )
        .await
        .unwrap(),
    );
    let secret = std::env::var("BETTER_AUTH_SECRET").unwrap();
    let keys = || {
        SecretKeys::new(
            BTreeMap::from([(0, secret.clone())]),
            0,
            Some(secret.clone()),
        )
        .unwrap()
    };
    let identity = Arc::new(
        IdentityService::new(
            db.clone(),
            CookiePolicy::new(false, secret.clone()).unwrap(),
        )
        .with_allocator(allocator.clone()),
    );
    let oauth = OAuthService::new(
        db.clone(),
        identity.clone(),
        allocator.clone(),
        Arc::new(keys()),
        OAuthConfig::new(
            "https://auth.example.invalid/api/auth",
            "https://studio.example.invalid/api/mcp",
            None,
        )
        .unwrap(),
    );
    let signing = oauth.provision_signing_key(&offline).await.unwrap();
    let email = format!(
        "oauth-tenant-{}@example.invalid",
        allocator.allocate().await.unwrap()
    );
    let user = identity
        .create_local_account(&email, "Tenant fixture", "fixture-password-123", false)
        .await
        .unwrap();
    db.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.\"user\" SET \"emailVerified\"=true,\"mustChangePassword\"=false WHERE id=$1",[user.clone().into()])).await.unwrap();
    let PasswordAuthentication::Authenticated(session) = identity
        .authenticate_password(&email, "fixture-password-123")
        .await
        .unwrap()
    else {
        panic!("session")
    };
    let mut customers = vec![];
    let mut organizations = vec![];
    let mut clients = vec![];
    for _ in 0..2 {
        let customer = allocator.allocate().await.unwrap().to_string();
        offline
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO webdock_admin.customers(id,name) VALUES($1,'OAuth tenant fixture')",
                [customer.clone().into()],
            ))
            .await
            .unwrap();
        let org: String = offline
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
                [customer.clone().into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "organization_id")
            .unwrap();
        let client = oauth
            .provision_client(
                &offline,
                ClientRegistration {
                    name: "Tenant OIDC".into(),
                    redirect_uris: vec!["https://site.example.invalid/api/sso/callback".into()],
                    post_logout_redirect_uris: vec![],
                    scopes: vec!["openid".into(), "profile".into(), "email".into()],
                    auth_method: ClientAuthMethod::None,
                    allow_refresh: false,
                    skip_consent: true,
                    purpose: ClientPurpose::Organization {
                        organization_id: org.clone(),
                    },
                },
            )
            .await
            .unwrap();
        customers.push(customer);
        organizations.push(org);
        clients.push(client);
    }
    let member = allocator.allocate().await.unwrap().to_string();
    offline.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES($1,$2,$3,'member',now())",[member.clone().into(),organizations[0].clone().into(),user.clone().into()])).await.unwrap();
    let grant = allocator.allocate().await.unwrap().to_string();
    offline.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.project_grant(id,user_id,binding_id,organization_id,role,enabled) VALUES($1,$2,$3,$4,'reader',true)",[grant.clone().into(),user.clone().into(),clients[0].binding_id.clone().unwrap().into(),organizations[0].clone().into()])).await.unwrap();
    let verifier = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ-._~";
    let challenge = oauth2::PkceCodeChallenge::from_code_verifier_sha256(
        &oauth2::PkceCodeVerifier::new(verifier.into()),
    )
    .as_str()
    .to_owned();
    let request = |index: usize| {
        AuthorizationRequest::code(
            clients[index].client_id.clone(),
            "https://site.example.invalid/api/sso/callback".into(),
            "openid profile email".into(),
            challenge.clone(),
        )
    };
    assert!(
        oauth
            .authorize(cookie(&session.set_cookie), request(1))
            .await
            .is_err(),
        "membership cannot cross tenants"
    );
    let AuthorizationOutcome::Redirect(location) = oauth
        .authorize(cookie(&session.set_cookie), request(0))
        .await
        .unwrap()
    else {
        panic!("redirect")
    };
    let code = url::Url::parse(&location)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .to_string();
    let auth = || ClientAuthentication {
        client_id: clients[0].client_id.clone(),
        client_secret: None,
        method: ClientAuthMethod::None,
    };
    let token = oauth
        .exchange_code(CodeExchange {
            client: auth(),
            code,
            redirect_uri: request(0).redirect_uri,
            code_verifier: verifier.into(),
        })
        .await
        .unwrap();
    assert_eq!(token.expires_in, 28800);
    assert_eq!(
        oauth
            .introspect(&auth(), &token.access_token)
            .await
            .unwrap()["webdock_role"],
        "reader"
    );
    assert_eq!(
        oauth.userinfo(&token.access_token).await.unwrap()["email"],
        email
    );
    offline
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_auth.project_grant SET enabled=false WHERE id=$1",
            [grant.clone().into()],
        ))
        .await
        .unwrap();
    assert_eq!(
        oauth
            .introspect(&auth(), &token.access_token)
            .await
            .unwrap()["active"],
        false
    );
    offline
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_auth.member SET role='admin' WHERE id=$1",
            [member.clone().into()],
        ))
        .await
        .unwrap();
    assert_eq!(
        oauth
            .introspect(&auth(), &token.access_token)
            .await
            .unwrap()["active"],
        false,
        "explicit revoked site grant overrides inherited admin"
    );
    offline
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.project_grant WHERE id=$1",
            [grant.into()],
        ))
        .await
        .unwrap();
    assert_eq!(
        oauth
            .introspect(&auth(), &token.access_token)
            .await
            .unwrap()["webdock_role"],
        "admin"
    );
    offline
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_admin.customers SET status='archived' WHERE id=$1",
            [customers[0].clone().into()],
        ))
        .await
        .unwrap();
    assert_eq!(
        oauth
            .introspect(&auth(), &token.access_token)
            .await
            .unwrap()["active"],
        false
    );
    for client in clients {
        offline
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM webdock_auth.app_binding WHERE id=$1",
                [client.binding_id.into()],
            ))
            .await
            .unwrap();
        offline
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM webdock_auth.\"oauthClient\" WHERE \"clientId\"=$1",
                [client.client_id.into()],
            ))
            .await
            .unwrap();
    }
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM webdock_auth.\"user\" WHERE id=$1",
        [user.into()],
    ))
    .await
    .unwrap();
    for (customer, org) in customers.into_iter().zip(organizations) {
        offline
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=$1",
                [customer.clone().into()],
            ))
            .await
            .unwrap();
        offline
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM webdock_admin.customers WHERE id=$1",
                [customer.into()],
            ))
            .await
            .unwrap();
        offline
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM webdock_auth.organization WHERE id=$1",
                [org.into()],
            ))
            .await
            .unwrap();
    }
    offline
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.jwks WHERE id=$1",
            [signing.into()],
        ))
        .await
        .unwrap();
    db.close().await.unwrap();
    offline.close().await.unwrap();
}
