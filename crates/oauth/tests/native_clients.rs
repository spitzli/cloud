//! Owned restricted PG18, disconnected legacy DB; no provider or production input.
#[path = "../../identity/tests/support/native_pg.rs"]
mod fixture;
use fixture::AUTH_SECRET;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use spitzli_identity::{CookiePolicy, IdentityService, SecretKeys};
use spitzli_oauth::{ClientAuthMethod, ClientAuthentication, Error, OAuthConfig, OAuthService};
use spitzli_persistence::{
    IdAllocator,
    postgres::{Limits, Pool},
};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
const REDIRECT: &str = "https://console.example.test/api/sso/callback";
fn credentials(secret: &str) -> ClientAuthentication {
    ClientAuthentication {
        client_id: "102".into(),
        client_secret: Some(secret.into()),
        method: ClientAuthMethod::ClientSecretPost,
    }
}
async fn setup() -> (fixture::Fixture, Arc<OAuthService>, Pool) {
    let f = fixture::Fixture::start().await;
    let hash = spitzli_oauth::legacy_token_hash("owned-client-secret");
    f.owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,r#"INSERT INTO webdock_auth."oauthClient"(id,"clientId","clientSecret",name,disabled,"skipConsent","enableEndSession",scopes,"redirectUris","postLogoutRedirectUris","grantTypes","responseTypes","tokenEndpointAuthMethod","requirePKCE","subjectType",metadata,"createdAt","updatedAt") VALUES('103','102',$1,'Owned native client',false,true,true,'["openid","profile","email"]','["https://console.example.test/api/sso/callback"]','[]','["authorization_code"]','["code"]','client_secret_post',true,'public','{"webdock_binding":"104"}',now(),now())"#,[hash.into()])).await.unwrap();
    f.owner.execute_unprepared("INSERT INTO webdock_auth.app_binding(id,client_id,label,organization_id,enabled) VALUES('104','102','Owned native client',null,true)").await.unwrap();
    let keys = Arc::new(
        SecretKeys::new(
            BTreeMap::from([(1, AUTH_SECRET.into())]),
            1,
            Some(AUTH_SECRET.into()),
        )
        .unwrap(),
    );
    let tx = f.owner.begin().await.unwrap();
    spitzli_oauth::stage_signing_key(&tx, "105", &keys)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let identity = Arc::new(IdentityService::new(
        DatabaseConnection::default(),
        CookiePolicy::new(true, AUTH_SECRET).unwrap(),
    ));
    let ids = Arc::new(
        IdAllocator::connect_native(
            &f.bootstrap_url,
            spitzli_ids::Identity::platform(0, 2).unwrap(),
            "webdock_auth_bootstrap",
            false,
        )
        .await
        .unwrap(),
    );
    let pool = Pool::connect(
        &f.bootstrap_url,
        "webdock_auth_bootstrap",
        false,
        Limits {
            max_connections: 1,
            min_connections: 0,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let oauth = OAuthService::new(
        DatabaseConnection::default(),
        identity,
        ids,
        keys,
        OAuthConfig::new(
            "https://auth.example.test/api/auth",
            "https://studio.example.test/api/mcp",
            Some("102".into()),
        )
        .unwrap(),
    )
    .with_native_client_store(pool.clone());
    (f, Arc::new(oauth), pool)
}
#[tokio::test]
#[ignore = "owned PG18; actual native client and public-key reads"]
async fn native_client_and_public_keys_work_without_legacy_database() {
    let (f, oauth, _) = setup().await;
    oauth
        .validate_portal_client(&credentials("owned-client-secret"), REDIRECT)
        .await
        .unwrap();
    assert!(matches!(
        oauth
            .validate_portal_client(&credentials("wrong-secret"), REDIRECT)
            .await,
        Err(Error::InvalidClient)
    ));
    assert!(
        oauth
            .validate_portal_client(
                &credentials("owned-client-secret"),
                "https://evil.example.test/api/sso/callback"
            )
            .await
            .is_err()
    );
    let mut wrong_method = credentials("owned-client-secret");
    wrong_method.method = ClientAuthMethod::ClientSecretBasic;
    assert!(matches!(
        oauth.validate_portal_client(&wrong_method, REDIRECT).await,
        Err(Error::InvalidClient)
    ));
    // An accidentally private JWK in the public column must still be normalized to public data.
    let stored = f
        .owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT \"privateKey\" FROM webdock_auth.jwks WHERE id='105'",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<String>("", "privateKey")
        .unwrap();
    let envelope: String = serde_json::from_str(&stored).unwrap();
    let keys = SecretKeys::new(
        BTreeMap::from([(1, AUTH_SECRET.into())]),
        1,
        Some(AUTH_SECRET.into()),
    )
    .unwrap();
    let raw = keys.decrypt(&envelope).unwrap();
    f.owner
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_auth.jwks SET \"publicKey\"=$1 WHERE id='105'",
            [raw.into()],
        ))
        .await
        .unwrap();
    let keys = oauth.jwks().await.unwrap();
    assert_eq!(keys["keys"].as_array().unwrap().len(), 1);
    let key = &keys["keys"][0];
    assert_eq!(key["kid"], "105");
    assert_eq!(key["alg"], "RS256");
    assert_eq!(key["use"], "sig");
    for name in ["d", "p", "q", "dp", "dq", "qi"] {
        assert!(key.get(name).is_none());
    }
    for (revoke, restore) in [
        (
            "UPDATE webdock_auth.\"oauthClient\" SET disabled=true",
            "UPDATE webdock_auth.\"oauthClient\" SET disabled=false",
        ),
        (
            "UPDATE webdock_auth.app_binding SET enabled=false",
            "UPDATE webdock_auth.app_binding SET enabled=true",
        ),
        (
            "UPDATE webdock_auth.\"oauthClient\" SET \"dpopBoundAccessTokens\"=true",
            "UPDATE webdock_auth.\"oauthClient\" SET \"dpopBoundAccessTokens\"=false",
        ),
        (
            "UPDATE webdock_auth.\"oauthClient\" SET \"subjectType\"='pairwise'",
            "UPDATE webdock_auth.\"oauthClient\" SET \"subjectType\"='public'",
        ),
        (
            "UPDATE webdock_auth.\"oauthClient\" SET scopes='[\"openid\",\"profile\"]'",
            "UPDATE webdock_auth.\"oauthClient\" SET scopes='[\"openid\",\"profile\",\"email\"]'",
        ),
    ] {
        f.owner.execute_unprepared(revoke).await.unwrap();
        assert!(
            oauth
                .validate_portal_client(&credentials("owned-client-secret"), REDIRECT)
                .await
                .is_err()
        );
        f.owner.execute_unprepared(restore).await.unwrap();
        oauth
            .validate_portal_client(&credentials("owned-client-secret"), REDIRECT)
            .await
            .unwrap();
    }
}
#[tokio::test]
#[ignore = "owned PG18; native caller retains the canonical client fence"]
async fn native_client_validation_holds_client_lock_until_caller_completion() {
    let (f, oauth, pool) = setup().await;
    let mut connection = pool.acquire().await.unwrap();
    let tx = connection.transaction().await.unwrap();
    let tx = oauth
        .validate_portal_client_native_in(tx, &credentials("owned-client-secret"), REDIRECT)
        .await
        .unwrap();
    let owner = f.owner.clone();
    let mutation = tokio::spawn(async move {
        owner
            .execute_unprepared(
                "UPDATE webdock_auth.\"oauthClient\" SET disabled=true WHERE \"clientId\"='102'",
            )
            .await
            .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!mutation.is_finished());
    tx.commit().await.unwrap();
    drop(connection);
    mutation.await.unwrap();
    assert!(
        oauth
            .validate_portal_client(&credentials("owned-client-secret"), REDIRECT)
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "owned PG18; binding changes during canonical client lock wait are observed"]
async fn native_client_validation_reads_binding_after_client_lock_wait() {
    let (f, oauth, _) = setup().await;
    let gate = f.owner.begin().await.unwrap();
    gate.execute_unprepared(
        "SELECT id FROM webdock_auth.\"oauthClient\" WHERE \"clientId\"='102' FOR UPDATE",
    )
    .await
    .unwrap();
    let task = tokio::spawn(async move {
        oauth
            .validate_portal_client(&credentials("owned-client-secret"), REDIRECT)
            .await
    });
    let until = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let waiting=f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name='spitzli-native-postgres' AND wait_event_type='Lock' AND query LIKE '%oauthClient%' AND query LIKE '%FOR SHARE%') AS blocked")).await.unwrap().unwrap().try_get::<bool>("","blocked").unwrap();
        if waiting {
            break;
        }
        assert!(tokio::time::Instant::now() < until);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    f.owner
        .execute_unprepared("UPDATE webdock_auth.app_binding SET enabled=false WHERE id='104'")
        .await
        .unwrap();
    gate.commit().await.unwrap();
    assert!(matches!(task.await.unwrap(), Err(Error::InvalidClient)));
    let rights=f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT has_column_privilege('webdock_auth_bootstrap','webdock_auth.app_binding','id','UPDATE') AS writable")).await.unwrap().unwrap().try_get::<bool>("","writable").unwrap();
    assert!(
        !rights,
        "client read port must not widen binding mutation privileges"
    );
}

#[tokio::test]
#[ignore = "owned PG18; native client disablement after row-lock wait cannot authorize"]
async fn native_client_eligibility_is_fresh_after_lock_wait() {
    let (f, oauth, _) = setup().await;
    let gate = f.owner.begin().await.unwrap();
    gate.execute_unprepared(
        "SELECT id FROM webdock_auth.\"oauthClient\" WHERE \"clientId\"='102' FOR UPDATE",
    )
    .await
    .unwrap();
    let task = tokio::spawn(async move {
        oauth
            .validate_portal_client(&credentials("owned-client-secret"), REDIRECT)
            .await
    });
    let until = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let waiting=f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name='spitzli-native-postgres' AND wait_event_type='Lock' AND query LIKE '%oauthClient%' AND query LIKE '%FOR SHARE%') AS blocked")).await.unwrap().unwrap().try_get::<bool>("","blocked").unwrap();
        if waiting {
            break;
        }
        assert!(tokio::time::Instant::now() < until);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    gate.execute_unprepared(
        "UPDATE webdock_auth.\"oauthClient\" SET disabled=true WHERE \"clientId\"='102'",
    )
    .await
    .unwrap();
    gate.commit().await.unwrap();
    assert!(matches!(task.await.unwrap(), Err(Error::InvalidClient)));
}
