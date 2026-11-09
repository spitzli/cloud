//! Actual owned OAuth issuance, disconnected retained reader; no provider/live input.
#[path = "../../identity/tests/support/native_pg.rs"]
mod fixture;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use fixture::AUTH_SECRET;
use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement, TransactionTrait,
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use webdock_identity::{CookiePolicy, IdentityService, SecretKeys};
use webdock_oauth::*;
use webdock_persistence::{
    IdAllocator,
    postgres::{Limits, Pool},
};
const ISSUER: &str = "https://auth.example.test/api/auth";
const RESOURCE: &str = "https://studio.example.test/api/mcp";
const REDIRECT: &str = "https://console.example.test/api/sso/callback";
struct Setup {
    f: fixture::Fixture,
    legacy: Arc<OAuthService>,
    native: Arc<OAuthService>,
    security: Arc<OAuthService>,
    grants: Arc<OAuthService>,
    pool: Pool,
    credentials: ClientAuthentication,
    auth_cookie: String,
    user: String,
    session: String,
    keys: Arc<SecretKeys>,
    key_id: String,
}
impl Setup {
    async fn start() -> Self {
        let f = fixture::Fixture::start().await;
        let db = Database::connect(&f.bootstrap_url).await.unwrap();
        let ids = Arc::new(
            IdAllocator::connect_native(
                &f.bootstrap_url,
                webdock_ids::Identity::platform(0, 2).unwrap(),
                "webdock_auth_bootstrap",
                false,
            )
            .await
            .unwrap(),
        );
        let keys = Arc::new(
            SecretKeys::new(
                BTreeMap::from([(1, AUTH_SECRET.into())]),
                1,
                Some(AUTH_SECRET.into()),
            )
            .unwrap(),
        );
        let identity = Arc::new(
            IdentityService::new(db.clone(), CookiePolicy::new(true, AUTH_SECRET).unwrap())
                .with_allocator(ids.clone()),
        );
        let config = |portal| OAuthConfig::new(ISSUER, RESOURCE, portal).unwrap();
        let bootstrap = OAuthService::new(
            db.clone(),
            identity.clone(),
            ids.clone(),
            keys.clone(),
            config(None),
        );
        let key_id = bootstrap.provision_signing_key(&f.owner).await.unwrap();
        let client = bootstrap
            .provision_client(
                &f.owner,
                ClientRegistration {
                    name: "Owned introspection".into(),
                    redirect_uris: vec![REDIRECT.into()],
                    post_logout_redirect_uris: vec![],
                    scopes: vec![
                        "openid".into(),
                        "profile".into(),
                        "email".into(),
                        "offline_access".into(),
                    ],
                    auth_method: ClientAuthMethod::ClientSecretPost,
                    allow_refresh: true,
                    skip_consent: true,
                    purpose: ClientPurpose::Portal,
                },
            )
            .await
            .unwrap();
        let credentials = ClientAuthentication {
            client_id: client.client_id.clone(),
            client_secret: client.client_secret,
            method: ClientAuthMethod::ClientSecretPost,
        };
        let user = ids.allocate().await.unwrap().to_string();
        let session = ids.allocate().await.unwrap().to_string();
        f.owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES($1,'Owned introspector','introspection@example.test',true,'user',false,false)",[user.clone().into()])).await.unwrap();
        f.owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES($1,'owned-introspection-token',$2,now()+interval '1 hour',now())",[session.clone().into(),user.clone().into()])).await.unwrap();
        f.owner.execute_unprepared("WITH hint AS MATERIALIZED(SELECT set_config('webdock.organization_allocation','{\"customer_id\":\"100\",\"organization_id\":\"101\"}',true)) INSERT INTO webdock_admin.customers(id,name) SELECT '100','Owned introspection' FROM hint").await.unwrap();
        f.owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES('owned-introspection-member','101',$1,'owner',now())",[user.clone().into()])).await.unwrap();
        f.owner.execute_unprepared("INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES('backup','Backup','backup@example.test',true,'user',false,false);INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES('backup-member','101','backup','owner',now())").await.unwrap();
        let legacy = Arc::new(OAuthService::new(
            db,
            identity.clone(),
            ids.clone(),
            keys.clone(),
            config(Some(credentials.client_id.clone())),
        ));
        let pool = Pool::connect(
            &f.bootstrap_url,
            "webdock_auth_bootstrap",
            false,
            Limits {
                max_connections: 2,
                min_connections: 0,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let native_identity = Arc::new(
            IdentityService::new(
                DatabaseConnection::default(),
                CookiePolicy::new(true, AUTH_SECRET).unwrap(),
            )
            .with_native_session_reads(pool.clone()),
        );
        let native = Arc::new(
            OAuthService::new(
                DatabaseConnection::default(),
                native_identity.clone(),
                ids.clone(),
                keys.clone(),
                config(Some(credentials.client_id.clone())),
            )
            .with_native_introspection(pool.clone()),
        );
        let security = Arc::new(
            OAuthService::new(
                DatabaseConnection::default(),
                native_identity.clone(),
                ids.clone(),
                keys.clone(),
                config(Some(credentials.client_id.clone())),
            )
            .with_native_security_store(pool.clone()),
        );
        let grants = Arc::new(
            OAuthService::new(
                DatabaseConnection::default(),
                native_identity,
                ids,
                keys.clone(),
                config(Some(credentials.client_id.clone())),
            )
            .with_native_grants_store(pool.clone()),
        );
        let auth_cookie = CookiePolicy::new(true, AUTH_SECRET)
            .unwrap()
            .header("owned-introspection-token", 3600)
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .into();
        Self {
            f,
            legacy,
            native,
            security,
            grants,
            pool,
            credentials,
            auth_cookie,
            user,
            session,
            keys,
            key_id,
        }
    }
    async fn issue(&self) -> TokenResponse {
        self.issue_for(
            &self.credentials,
            "openid profile email offline_access",
            vec![],
        )
        .await
    }
    async fn issue_for(
        &self,
        credentials: &ClientAuthentication,
        scope: &str,
        resources: Vec<String>,
    ) -> TokenResponse {
        let verifier = "v".repeat(64);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut request = AuthorizationRequest::code(
            credentials.client_id.clone(),
            REDIRECT.into(),
            scope.into(),
            challenge,
        );
        request.resources = resources;
        request.nonce = Some("owned-nonce".into());
        let AuthorizationOutcome::Redirect(url) = self
            .legacy
            .authorize(&self.auth_cookie, request)
            .await
            .unwrap()
        else {
            panic!("owned code expected")
        };
        let url = url::Url::parse(&url).unwrap();
        let code = url
            .query_pairs()
            .find(|(k, _)| k == "code")
            .unwrap()
            .1
            .into_owned();
        self.legacy
            .exchange_code(CodeExchange {
                client: credentials.clone(),
                code,
                redirect_uri: REDIRECT.into(),
                code_verifier: verifier,
            })
            .await
            .unwrap()
    }
}
#[tokio::test]
#[ignore = "owned PG18; native opaque/refresh/portal/userinfo parity with disconnected retained reader"]
async fn native_introspection_preserves_actual_oauth_token_contracts() {
    let s = Setup::start().await;
    let tokens = s.issue().await;
    assert_eq!(
        s.native
            .introspect(&s.credentials, &tokens.access_token)
            .await
            .unwrap(),
        s.legacy
            .introspect(&s.credentials, &tokens.access_token)
            .await
            .unwrap()
    );
    assert_eq!(
        s.native
            .introspect_portal(&s.credentials, &tokens.access_token)
            .await
            .unwrap(),
        s.legacy
            .introspect_portal(&s.credentials, &tokens.access_token)
            .await
            .unwrap()
    );
    let refresh = tokens.refresh_token.as_ref().unwrap();
    assert_eq!(
        s.native.introspect(&s.credentials, refresh).await.unwrap(),
        s.legacy.introspect(&s.credentials, refresh).await.unwrap()
    );
    assert_eq!(
        s.native
            .introspect_portal(&s.credentials, refresh)
            .await
            .unwrap()["active"],
        false
    );
    assert_eq!(
        s.native.userinfo(&tokens.access_token).await.unwrap(),
        s.legacy.userinfo(&tokens.access_token).await.unwrap()
    );
    let mut wrong = s.credentials.clone();
    wrong.client_secret = Some("wrong".into());
    assert!(
        s.native
            .introspect(&wrong, &tokens.access_token)
            .await
            .is_err()
    );
    s.legacy
        .revoke(&s.credentials, &tokens.access_token)
        .await
        .unwrap();
    assert_eq!(
        s.native
            .introspect(&s.credentials, &tokens.access_token)
            .await
            .unwrap()["active"],
        false
    );
}

impl Setup {
    async fn sql(&self, sql: &str) {
        self.f.owner.execute_unprepared(sql).await.unwrap();
    }
    async fn sign(&self, claims: &serde_json::Value) -> String {
        let row = self
            .f
            .owner
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT \"privateKey\" FROM webdock_auth.jwks WHERE id=$1",
                [self.key_id.clone().into()],
            ))
            .await
            .unwrap()
            .unwrap();
        let envelope: String =
            serde_json::from_str(&row.try_get::<String>("", "privateKey").unwrap()).unwrap();
        let private = self.keys.decrypt(&envelope).unwrap();
        let key = josekit::jwk::Jwk::from_bytes(private.as_bytes()).unwrap();
        let mut header = josekit::jws::JwsHeader::new();
        header.set_algorithm("RS256");
        header.set_key_id(&self.key_id);
        josekit::jws::serialize_compact(
            &serde_json::to_vec(claims).unwrap(),
            &header,
            &josekit::jws::RS256.signer_from_jwk(&key).unwrap(),
        )
        .unwrap()
    }
    async fn active(&self, token: &str) -> serde_json::Value {
        self.native
            .introspect(&self.credentials, token)
            .await
            .unwrap()
    }
    async fn wait_lock(&self, table: &str) {
        let until = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let row=self.f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND application_name='spitzli-native-postgres' AND wait_event_type='Lock' AND query LIKE $1) AS blocked",[format!("%{table}%").into()])).await.unwrap().unwrap();
            if row.try_get::<bool>("", "blocked").unwrap() {
                break;
            }
            assert!(
                tokio::time::Instant::now() < until,
                "native lock not observed"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
#[tokio::test]
#[ignore = "owned PG18; canonical tenant/provider/configuration and refresh replay authority"]
async fn native_introspection_observes_current_policy_and_refresh_replay() {
    let s = Setup::start().await;
    let t = s.issue().await;

    for (deny, restore) in [
        (
            "UPDATE webdock_admin.customers SET status='archived'",
            "UPDATE webdock_admin.customers SET status='active'",
        ),
        (
            "UPDATE webdock_auth.app_binding SET enabled=false",
            "UPDATE webdock_auth.app_binding SET enabled=true",
        ),
        (
            "UPDATE webdock_auth.\"user\" SET banned=true WHERE id<>'backup'",
            "UPDATE webdock_auth.\"user\" SET banned=false WHERE id<>'backup'",
        ),
        (
            "UPDATE webdock_auth.session SET \"expiresAt\"=now()-interval '1 second'",
            "UPDATE webdock_auth.session SET \"expiresAt\"=now()+interval '1 hour'",
        ),
        (
            "UPDATE webdock_auth.\"oauthAccessToken\" SET \"expiresAt\"=now()-interval '1 second'",
            "UPDATE webdock_auth.\"oauthAccessToken\" SET \"expiresAt\"=now()+interval '1 hour'",
        ),
    ] {
        s.sql(deny).await;
        let native = s.active(&t.access_token).await;
        let retained = s
            .legacy
            .introspect(&s.credentials, &t.access_token)
            .await
            .unwrap();
        {
            assert_eq!(native["active"], false, "{deny}");
            assert_eq!(native, retained, "{deny}");
        }
        s.sql(restore).await;
        assert_eq!(s.active(&t.access_token).await["active"], true, "{restore}");
    }
    s.sql("DELETE FROM webdock_auth.member WHERE id<>'backup-member'")
        .await;
    assert_eq!(s.active(&t.access_token).await["active"], false);
    s.sql(&format!("INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES('restored','101','{}','owner',now())",s.user)).await;
    let refresh = t.refresh_token.unwrap();
    let next = s
        .legacy
        .refresh(RefreshExchange {
            client: s.credentials.clone(),
            refresh_token: refresh.clone(),
            scope: None,
        })
        .await
        .unwrap();
    assert_eq!(s.active(&refresh).await["active"], false);
    assert_eq!(s.active(&next.access_token).await["active"], true);
    assert!(
        s.legacy
            .refresh(RefreshExchange {
                client: s.credentials.clone(),
                refresh_token: refresh,
                scope: None
            })
            .await
            .is_err()
    );
    assert_eq!(s.active(&next.access_token).await["active"], false);
}
#[tokio::test]
#[ignore = "owned PG18; signed compatibility tokens preserve current authority and revocation"]
async fn native_legacy_jwt_is_not_an_id_token_or_tombstone_escape() {
    use serde_json::json;
    let s = Setup::start().await;
    let t = s.issue().await;
    assert_eq!(
        s.active(t.id_token.as_ref().unwrap()).await["active"],
        false
    );
    let now = chrono::Utc::now().timestamp();
    let claims = json!({"iss":ISSUER,"aud":format!("{ISSUER}/oauth2/userinfo"),"azp":s.credentials.client_id,"sub":s.user,"sid":s.session,"scope":"openid profile email","iat":now,"exp":now+300});
    let token = s.sign(&claims).await;
    assert_eq!(
        s.active(&token).await,
        s.legacy.introspect(&s.credentials, &token).await.unwrap()
    );
    assert_eq!(s.active(&token).await["active"], true);
    for (field, value) in [
        ("iss", json!("https://wrong.test")),
        ("cnf", json!({"jkt":"unhandled"})),
        ("aud", json!("https://ungranted.test")),
        ("exp", json!(now - 1)),
        ("iat", json!(now + 3600)),
        ("scope", json!("openid hosting:write")),
    ] {
        let mut invalid = claims.clone();
        invalid[field] = value;
        let invalid = s.sign(&invalid).await;
        assert_eq!(s.active(&invalid).await["active"], false, "{field}");
    }
    let mut damaged = token.clone().into_bytes();
    let n = damaged.len();
    damaged[n - 5] = if damaged[n - 5] == b'A' { b'B' } else { b'A' };
    assert_eq!(
        s.active(std::str::from_utf8(&damaged).unwrap()).await["active"],
        false
    );
    let hash = legacy_token_hash(&token);
    s.sql(&format!("INSERT INTO webdock_auth.\"oauthAccessToken\"(id,token,\"clientId\",\"sessionId\",\"userId\",scopes,resources,\"expiresAt\",\"createdAt\") VALUES('jwt-discovery','{hash}','{}','{}','{}','[\"openid\",\"profile\",\"email\"]','[]',now()+interval '1 hour',now())",s.credentials.client_id,s.session,s.user)).await;
    let gate = s.f.owner.begin().await.unwrap();
    gate.execute_unprepared(
        "SELECT id FROM webdock_auth.\"oauthAccessToken\" WHERE id='jwt-discovery' FOR UPDATE",
    )
    .await
    .unwrap();
    let native = s.native.clone();
    let credentials = s.credentials.clone();
    let jwt = token.clone();
    let task = tokio::spawn(async move { native.introspect(&credentials, &jwt).await });
    s.wait_lock("oauthAccessToken").await;
    gate.execute_unprepared(
        "DELETE FROM webdock_auth.\"oauthAccessToken\" WHERE id='jwt-discovery'",
    )
    .await
    .unwrap();
    gate.commit().await.unwrap();
    assert_eq!(
        task.await.unwrap().unwrap()["active"],
        false,
        "a disappearing discovered JWT-shaped record cannot fall back"
    );
    assert_eq!(
        s.active(&token).await["active"],
        true,
        "only initial absence may use legacy JWT compatibility"
    );
    s.legacy.revoke(&s.credentials, &token).await.unwrap();
    assert_eq!(s.active(&token).await["active"], false);
}
#[tokio::test]
#[ignore = "owned PG18; native caller-owned identity/client/token fences and cancellation"]
async fn native_introspection_holds_fences_until_caller_commit() {
    let s = Setup::start().await;
    let t = s.issue().await;
    let mut connection = s.pool.acquire().await.unwrap();
    let tx = connection.transaction().await.unwrap();
    let (tx, claims) = s
        .native
        .introspect_native_in(tx, &s.credentials, &t.access_token)
        .await
        .unwrap();
    assert_eq!(claims["active"], true);
    for sql in [
        "SELECT id FROM webdock_auth.\"oauthClient\" FOR UPDATE NOWAIT",
        "SELECT id FROM webdock_auth.\"user\" FOR UPDATE NOWAIT",
        "SELECT id FROM webdock_auth.session FOR UPDATE NOWAIT",
        "SELECT id FROM webdock_auth.\"oauthAccessToken\" FOR UPDATE NOWAIT",
    ] {
        let gate = s.f.owner.begin().await.unwrap();
        let error = gate
            .query_all_raw(Statement::from_string(DbBackend::Postgres, sql))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("55P03")
                || error.to_string().contains("could not obtain lock"),
            "{error}"
        );
        gate.rollback().await.unwrap();
    }
    tx.commit().await.unwrap();
    drop(connection);
    let gate = s.f.owner.begin().await.unwrap();
    gate.execute_unprepared("SELECT id FROM webdock_auth.session FOR UPDATE")
        .await
        .unwrap();
    let native = s.native.clone();
    let credentials = s.credentials.clone();
    let token = t.access_token.clone();
    let task = tokio::spawn(async move { native.introspect(&credentials, &token).await });
    s.wait_lock("session").await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    gate.commit().await.unwrap();
    assert_eq!(
        s.active(&t.access_token).await["active"],
        true,
        "canceled checked connection cannot poison reuse"
    );
}
#[tokio::test]
#[ignore = "owned PG18; fresh expiry and immutable discovery after ownership lock wait"]
async fn native_introspection_cannot_revive_expired_or_replaced_discovery() {
    let s = Setup::start().await;
    let t = s.issue().await;
    let gate = s.f.owner.begin().await.unwrap();
    gate.execute_unprepared("SELECT id FROM webdock_auth.\"oauthAccessToken\" FOR UPDATE")
        .await
        .unwrap();
    let native = s.native.clone();
    let credentials = s.credentials.clone();
    let token = t.access_token.clone();
    let task = tokio::spawn(async move { native.introspect(&credentials, &token).await });
    s.wait_lock("oauthAccessToken").await;
    gate.execute_unprepared("UPDATE webdock_auth.\"oauthAccessToken\" SET \"expiresAt\"=clock_timestamp()-interval '1 second'").await.unwrap();
    gate.commit().await.unwrap();
    assert_eq!(task.await.unwrap().unwrap()["active"], false);
    s.sql("UPDATE webdock_auth.\"oauthAccessToken\" SET \"expiresAt\"=clock_timestamp()+interval '1 hour'").await;
    let gate = s.f.owner.begin().await.unwrap();
    gate.execute_unprepared("SELECT id FROM webdock_auth.\"oauthAccessToken\" FOR UPDATE")
        .await
        .unwrap();
    let native = s.native.clone();
    let credentials = s.credentials.clone();
    let token = t.access_token.clone();
    let task = tokio::spawn(async move { native.introspect(&credentials, &token).await });
    s.wait_lock("oauthAccessToken").await;
    gate.execute_unprepared("UPDATE webdock_auth.\"oauthAccessToken\" SET \"sessionId\"=NULL")
        .await
        .unwrap();
    gate.commit().await.unwrap();
    assert_eq!(task.await.unwrap().unwrap()["active"], false);
}

#[tokio::test]
#[ignore = "owned PG18; first-party preview does not escape into public introspection/userinfo"]
async fn native_introspection_keeps_portal_preview_narrow() {
    let s = Setup::start().await;
    let t = s.issue().await;
    s.sql("UPDATE webdock_auth.\"user\" SET role='operator',\"twoFactorEnabled\"=true WHERE id<>'backup'")
        .await;
    s.sql(&format!("INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id) VALUES('{}','{}','100','101')",s.session,s.user)).await;
    for mode in ["reader", "owner"] {
        if mode == "owner" {
            s.sql("UPDATE webdock_auth.studio_tenant_preview SET mode='owner'")
                .await;
        }
        assert_eq!(s.active(&t.access_token).await["active"], false);
        let claims = s
            .native
            .introspect_portal(&s.credentials, &t.access_token)
            .await
            .unwrap();
        assert_eq!(
            claims,
            s.legacy
                .introspect_portal(&s.credentials, &t.access_token)
                .await
                .unwrap()
        );
        assert_eq!(claims["active"], true);
        assert_eq!(claims["webdock_preview"], true);
        assert_eq!(
            claims["webdock_role"],
            if mode == "owner" { "admin" } else { "reader" }
        );
        assert!(s.native.userinfo(&t.access_token).await.is_err());
    }
}
#[tokio::test]
#[ignore = "owned PG18; native MCP scope/audience, cross-client grants and retained resource fences"]
async fn native_mcp_introspection_observes_resource_grants() {
    let s = Setup::start().await;
    let c = s
        .legacy
        .provision_client(
            &s.f.owner,
            ClientRegistration {
                name: "Owned hosting".into(),
                redirect_uris: vec![REDIRECT.into()],
                post_logout_redirect_uris: vec![],
                scopes: vec![
                    "openid".into(),
                    "hosting:read".into(),
                    "offline_access".into(),
                ],
                auth_method: ClientAuthMethod::ClientSecretPost,
                allow_refresh: true,
                skip_consent: true,
                purpose: ClientPurpose::McpHosting,
            },
        )
        .await
        .unwrap();
    let c = ClientAuthentication {
        client_id: c.client_id,
        client_secret: c.client_secret,
        method: ClientAuthMethod::ClientSecretPost,
    };
    let t = s
        .issue_for(
            &c,
            "openid hosting:read offline_access",
            vec![RESOURCE.into()],
        )
        .await;
    let claims = s.native.introspect_mcp(&c, &t.access_token).await.unwrap();
    assert_eq!(
        claims,
        s.legacy.introspect_mcp(&c, &t.access_token).await.unwrap()
    );
    assert_eq!(claims["webdock_role"], "hosting-customer");
    assert_eq!(claims["active"], true);
    assert_eq!(
        s.active(&t.access_token).await["active"],
        false,
        "unlinked caller cannot introspect another client"
    );
    s.sql(&format!("INSERT INTO webdock_auth.\"oauthClientResource\"(id,\"clientId\",\"resourceId\",\"createdAt\") VALUES('owned-caller-grant','{}','{RESOURCE}',now())",s.credentials.client_id)).await;
    assert_eq!(s.active(&t.access_token).await["active"], true);
    assert_eq!(
        s.active(t.refresh_token.as_ref().unwrap()).await["active"],
        false,
        "cross-client refresh remains private"
    );
    let mut connection = s.pool.acquire().await.unwrap();
    let tx = connection.transaction().await.unwrap();
    let (tx, value) = s
        .native
        .introspect_native_in(tx, &s.credentials, &t.access_token)
        .await
        .unwrap();
    assert_eq!(value["active"], true);
    for sql in [
        "SELECT id FROM webdock_auth.\"oauthResource\" FOR UPDATE NOWAIT",
        "SELECT id FROM webdock_auth.\"oauthClientResource\" FOR UPDATE NOWAIT",
    ] {
        let gate = s.f.owner.begin().await.unwrap();
        let error = gate
            .query_all_raw(Statement::from_string(DbBackend::Postgres, sql))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("could not obtain lock"),
            "{error}"
        );
        gate.rollback().await.unwrap();
    }
    tx.commit().await.unwrap();
    drop(connection);
    let gate = s.f.owner.begin().await.unwrap();
    gate.execute_unprepared("SELECT id FROM webdock_auth.\"oauthResource\" FOR UPDATE")
        .await
        .unwrap();
    s.sql("UPDATE webdock_auth.\"oauthAccessToken\" SET \"expiresAt\"=clock_timestamp()+interval '1 second'").await;
    let native = s.native.clone();
    let c2 = c.clone();
    let token = t.access_token.clone();
    let task = tokio::spawn(async move { native.introspect_mcp(&c2, &token).await });
    s.wait_lock("oauthResource").await;
    gate.execute_unprepared("SELECT pg_sleep(1.1)")
        .await
        .unwrap();
    gate.commit().await.unwrap();
    assert_eq!(
        task.await.unwrap().unwrap()["active"],
        false,
        "expiry after resource lock wait must be current"
    );
    s.sql("UPDATE webdock_auth.\"oauthAccessToken\" SET \"expiresAt\"=clock_timestamp()+interval '1 hour'").await;
    for (deny, restore) in [
        (
            "UPDATE webdock_auth.\"oauthResource\" SET disabled=true",
            "UPDATE webdock_auth.\"oauthResource\" SET disabled=false",
        ),
        (
            "UPDATE webdock_auth.\"oauthResource\" SET \"allowedScopes\"='[]'",
            "UPDATE webdock_auth.\"oauthResource\" SET \"allowedScopes\"='[\"hosting:read\",\"offline_access\"]'",
        ),
    ] {
        s.sql(deny).await;
        assert_eq!(
            s.native.introspect_mcp(&c, &t.access_token).await.unwrap()["active"],
            false
        );
        s.sql(restore).await;
    }
    s.sql("DELETE FROM webdock_auth.\"oauthClientResource\" WHERE id='owned-caller-grant'")
        .await;
    assert_eq!(s.active(&t.access_token).await["active"], false);
}

#[tokio::test]
#[ignore = "owned PG18; provider credential/session and uncertainty remain authoritative"]
async fn native_introspection_rechecks_provider_and_fails_closed() {
    let s = Setup::start().await;
    let t = s.issue().await;
    s.sql(r#"CREATE SCHEMA neon_auth;CREATE TABLE neon_auth."user"(id uuid PRIMARY KEY,banned boolean);CREATE TABLE neon_auth.session(id uuid PRIMARY KEY,"userId" uuid,"expiresAt" timestamptz,"impersonatedBy" uuid);CREATE TABLE neon_auth.account("userId" uuid,"providerId" text,password text);GRANT USAGE ON SCHEMA neon_auth TO webdock_platform_owner;GRANT SELECT ON ALL TABLES IN SCHEMA neon_auth TO webdock_platform_owner;
    INSERT INTO neon_auth."user" VALUES('00000000-0000-4000-8000-000000000001',false);INSERT INTO neon_auth.session VALUES('00000000-0000-4000-8000-000000000002','00000000-0000-4000-8000-000000000001',now()+interval '1 hour',null);INSERT INTO neon_auth.account VALUES('00000000-0000-4000-8000-000000000001','credential','owned-hash')"#).await;
    s.sql(&format!("INSERT INTO webdock_auth.neon_identity(user_id,provider_origin,provider_subject,linked_by) VALUES('{}','https://provider.example.test/auth','00000000-0000-4000-8000-000000000001','{}');INSERT INTO webdock_auth.neon_session(session_id,user_id,provider_session_id,provider_subject,identity_revision,credential_digest,encrypted_cookie,mfa_verified) VALUES('{}','{}','00000000-0000-4000-8000-000000000002','00000000-0000-4000-8000-000000000001',1,encode(sha256(convert_to('owned-hash','UTF8')),'hex'),'owned-custody',false)",s.user,s.user,s.session,s.user)).await;
    assert_eq!(s.active(&t.access_token).await["active"], true);
    for (deny, restore) in [
        (
            "UPDATE neon_auth.session SET \"expiresAt\"=now()-interval '1 second'",
            "UPDATE neon_auth.session SET \"expiresAt\"=now()+interval '1 hour'",
        ),
        (
            "UPDATE neon_auth.account SET password='changed-hash'",
            "UPDATE neon_auth.account SET password='owned-hash'",
        ),
        (
            "UPDATE neon_auth.\"user\" SET banned=true",
            "UPDATE neon_auth.\"user\" SET banned=false",
        ),
    ] {
        s.sql(deny).await;
        assert_eq!(s.active(&t.access_token).await["active"], false);
        s.sql(restore).await;
        assert_eq!(s.active(&t.access_token).await["active"], true);
    }
    s.sql("ALTER FUNCTION webdock_rls.native_provider_session_live(text,text) RENAME TO owned_missing_authority").await;
    assert!(
        matches!(
            s.native.introspect(&s.credentials, &t.access_token).await,
            Err(Error::Identity(_))
        ),
        "no retained fallback for missing native authority"
    );
}

#[tokio::test]
#[ignore = "owned PG18; CMS project grants, tenant authority and global admission"]
async fn native_introspection_preserves_organization_project_grants() {
    let s = Setup::start().await;
    let c = s
        .legacy
        .provision_client(
            &s.f.owner,
            ClientRegistration {
                name: "Owned org site".into(),
                redirect_uris: vec![REDIRECT.into()],
                post_logout_redirect_uris: vec![],
                scopes: vec!["openid".into(), "profile".into(), "email".into()],
                auth_method: ClientAuthMethod::ClientSecretPost,
                allow_refresh: false,
                skip_consent: true,
                purpose: ClientPurpose::Organization {
                    organization_id: "101".into(),
                },
            },
        )
        .await
        .unwrap();
    let binding = c.binding_id.unwrap();
    let c = ClientAuthentication {
        client_id: c.client_id,
        client_secret: c.client_secret,
        method: ClientAuthMethod::ClientSecretPost,
    };
    let t = s.issue_for(&c, "openid profile email", vec![]).await;
    let claims = s.native.introspect(&c, &t.access_token).await.unwrap();
    assert_eq!(
        claims,
        s.legacy.introspect(&c, &t.access_token).await.unwrap()
    );
    assert_eq!(claims["webdock_role"], "admin");
    s.sql(&format!("INSERT INTO webdock_auth.project_grant(id,user_id,binding_id,organization_id,role,enabled) VALUES('owned-grant','{}','{binding}','101','reader',false)",s.user)).await;
    assert_eq!(
        s.native.introspect(&c, &t.access_token).await.unwrap()["active"],
        false,
        "explicit disabled grant overrides owner"
    );
    s.sql("UPDATE webdock_auth.project_grant SET enabled=true")
        .await;
    s.sql("UPDATE webdock_auth.member SET role='member' WHERE id<>'backup-member'")
        .await;
    let claims = s.native.introspect(&c, &t.access_token).await.unwrap();
    assert_eq!(
        claims,
        s.legacy.introspect(&c, &t.access_token).await.unwrap()
    );
    assert_eq!(claims["webdock_role"], "reader");
    s.sql("INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,'{\"cmsEnabled\":false}') ON CONFLICT(id) DO UPDATE SET settings=excluded.settings").await;
    assert_eq!(
        s.native.introspect(&c, &t.access_token).await.unwrap()["active"],
        false
    );
}

#[tokio::test]
#[ignore = "owned PG18; native security writes with disconnected retained writer"]
async fn native_security_revokes_real_access_refresh_and_legacy_jwt() {
    use serde_json::json;
    let s = Setup::start().await;
    let t = s.issue().await;
    s.security
        .revoke(&s.credentials, &t.access_token)
        .await
        .unwrap();
    assert_eq!(s.active(&t.access_token).await["active"], false);
    assert_eq!(
        s.active(t.refresh_token.as_ref().unwrap()).await["active"],
        true
    );
    s.security
        .revoke(&s.credentials, t.refresh_token.as_ref().unwrap())
        .await
        .unwrap();
    assert_eq!(
        s.active(t.refresh_token.as_ref().unwrap()).await["active"],
        false
    );
    s.security
        .revoke(&s.credentials, "unknown-token")
        .await
        .unwrap();
    s.security
        .revoke(&s.credentials, &t.access_token)
        .await
        .unwrap();
    let now = chrono::Utc::now().timestamp();
    let jwt=s.sign(&json!({"iss":ISSUER,"aud":format!("{ISSUER}/oauth2/userinfo"),"azp":s.credentials.client_id,"sub":s.user,"sid":s.session,"scope":"openid profile email","iat":now,"exp":now+300})).await;
    assert_eq!(s.active(&jwt).await["active"], true);
    s.security.revoke(&s.credentials, &jwt).await.unwrap();
    assert_eq!(s.active(&jwt).await["active"], false);
}

#[tokio::test]
#[ignore = "owned PG18; a canonical subject appearing after absence must not be used unfenced"]
async fn native_security_refuses_a_subject_created_after_its_fence_check() {
    use serde_json::json;
    let s = Setup::start().await;
    let now = chrono::Utc::now().timestamp();
    let jwt=s.sign(&json!({"iss":ISSUER,"aud":format!("{ISSUER}/oauth2/userinfo"),"azp":s.credentials.client_id,"sub":"000-new-user","sid":s.session,"scope":"openid","iat":now,"exp":now+300})).await;
    let gate = s.f.owner.begin().await.unwrap();
    gate.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM webdock_auth.\"user\" WHERE id=$1 FOR UPDATE",
        [s.user.clone().into()],
    ))
    .await
    .unwrap();
    let security = s.security.clone();
    let c = s.credentials.clone();
    let j = jwt.clone();
    let task = tokio::spawn(async move { security.revoke(&c, &j).await });
    s.wait_lock("user").await;
    s.sql("INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES('000-new-user','New','new@example.test',true,'user',false,false)").await;
    gate.commit().await.unwrap();
    assert!(
        matches!(task.await.unwrap(), Err(Error::Configuration)),
        "a newly appearing subject was not protected by the earlier owner fence"
    );
}

#[tokio::test]
#[ignore = "owned PG18; native revocation cannot mutate foreign client records or bypass credentials"]
async fn native_security_preserves_client_ownership_and_invalid_hint_semantics() {
    let s = Setup::start().await;
    let t = s.issue().await;
    let c = s
        .legacy
        .provision_client(
            &s.f.owner,
            ClientRegistration {
                name: "Other revoker".into(),
                redirect_uris: vec![REDIRECT.into()],
                post_logout_redirect_uris: vec![],
                scopes: vec!["openid".into()],
                auth_method: ClientAuthMethod::ClientSecretPost,
                allow_refresh: false,
                skip_consent: true,
                purpose: ClientPurpose::Portal,
            },
        )
        .await
        .unwrap();
    let c = ClientAuthentication {
        client_id: c.client_id,
        client_secret: c.client_secret,
        method: ClientAuthMethod::ClientSecretPost,
    };
    s.security.revoke(&c, &t.access_token).await.unwrap();
    assert_eq!(s.active(&t.access_token).await["active"], true);
    let mut wrong = s.credentials.clone();
    wrong.client_secret = Some("wrong".into());
    assert!(matches!(
        s.security.revoke(&wrong, &t.access_token).await,
        Err(Error::InvalidClient)
    ));
    assert_eq!(s.active(&t.access_token).await["active"], true);
    for invalid in ["malformed.jwt.hint", t.id_token.as_ref().unwrap()] {
        s.security.revoke(&s.credentials, invalid).await.unwrap();
    }
    assert_eq!(
        s.active(&t.access_token).await["active"],
        true,
        "ID-token revocation does not end the canonical session"
    );
}
#[tokio::test]
#[ignore = "owned PG18; native logout exact cookie/hint identity and redirect, expired signed hints allowed"]
async fn native_security_logout_requires_exact_identity_and_redirect() {
    let s = Setup::start().await;
    let t = s.issue().await;
    let request = || LogoutRequest {
        id_token_hint: t.id_token.clone().unwrap(),
        post_logout_redirect_uri: None,
        state: None,
    };
    assert!(matches!(
        s.security.end_session("", request()).await.unwrap(),
        LogoutOutcome::ConfirmationRequired
    ));
    let mut bad = request();
    bad.id_token_hint = "bad.hint.signature".into();
    assert!(matches!(
        s.security.end_session(&s.auth_cookie, bad).await.unwrap(),
        LogoutOutcome::ConfirmationRequired
    ));
    let mut bad = request();
    bad.post_logout_redirect_uri = Some("https://evil.example.test".into());
    assert!(matches!(
        s.security.end_session(&s.auth_cookie, bad).await,
        Err(Error::InvalidRequest)
    ));
    let now = chrono::Utc::now().timestamp();
    let wrong=s.sign(&serde_json::json!({"iss":ISSUER,"aud":s.credentials.client_id,"sub":"backup","sid":s.session,"iat":now,"exp":now+300})).await;
    assert!(matches!(
        s.security
            .end_session(
                &s.auth_cookie,
                LogoutRequest {
                    id_token_hint: wrong,
                    post_logout_redirect_uri: None,
                    state: None
                }
            )
            .await
            .unwrap(),
        LogoutOutcome::ConfirmationRequired
    ));
    assert_eq!(s.active(&t.access_token).await["active"], true);
    s.sql("UPDATE webdock_auth.\"oauthClient\" SET \"postLogoutRedirectUris\"='[\"https://console.example.test/login\"]'").await;
    let expired=s.sign(&serde_json::json!({"iss":ISSUER,"aud":s.credentials.client_id,"sub":s.user,"sid":s.session,"iat":now-600,"exp":now-300})).await;
    let outcome = s
        .security
        .end_session(
            &s.auth_cookie,
            LogoutRequest {
                id_token_hint: expired,
                post_logout_redirect_uri: Some("https://console.example.test/login".into()),
                state: Some("owned state".into()),
            },
        )
        .await
        .unwrap();
    let LogoutOutcome::Complete {
        redirect,
        set_cookie,
    } = outcome
    else {
        panic!("verified own logout must complete")
    };
    let redirect = url::Url::parse(redirect.as_ref().unwrap()).unwrap();
    assert_eq!(
        redirect.origin().ascii_serialization(),
        "https://console.example.test"
    );
    assert_eq!(
        redirect
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1,
        "owned state"
    );
    assert!(set_cookie.contains("Max-Age=0"));
    assert_eq!(s.active(&t.access_token).await["active"], false);
}
#[tokio::test]
#[ignore = "owned PG18; logout serializes exact canonical owner, including setup-only sessions"]
async fn native_security_parallel_logout_and_setup_session_are_safe() {
    let s = Setup::start().await;
    let t = s.issue().await;
    s.sql("UPDATE webdock_auth.\"user\" SET \"mustChangePassword\"=true WHERE id<>'backup'")
        .await;
    let request = || LogoutRequest {
        id_token_hint: t.id_token.clone().unwrap(),
        post_logout_redirect_uri: None,
        state: None,
    };
    let (a, b) = tokio::join!(
        s.security.end_session(&s.auth_cookie, request()),
        s.security.end_session(&s.auth_cookie, request())
    );
    let outcomes = [a.unwrap(), b.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|o| matches!(o, LogoutOutcome::Complete { .. }))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|o| matches!(o, LogoutOutcome::ConfirmationRequired))
            .count(),
        1
    );
}
#[tokio::test]
#[ignore = "owned PG18; revocation fails closed on immutable owner change and cancels safely"]
async fn native_security_rechecks_token_ownership_and_cancellation() {
    let s = Setup::start().await;
    let t = s.issue().await;
    let gate = s.f.owner.begin().await.unwrap();
    gate.execute_unprepared("SELECT id FROM webdock_auth.\"oauthAccessToken\" FOR UPDATE")
        .await
        .unwrap();
    let security = s.security.clone();
    let c = s.credentials.clone();
    let token = t.access_token.clone();
    let task = tokio::spawn(async move { security.revoke(&c, &token).await });
    s.wait_lock("oauthAccessToken").await;
    gate.execute_unprepared("UPDATE webdock_auth.\"oauthAccessToken\" SET \"sessionId\"=NULL")
        .await
        .unwrap();
    gate.commit().await.unwrap();
    assert!(matches!(task.await.unwrap(), Err(Error::Configuration)));
    let row = s
        .f
        .owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT revoked IS NULL AS untouched FROM webdock_auth.\"oauthAccessToken\" LIMIT 1",
        ))
        .await
        .unwrap()
        .unwrap();
    assert!(row.try_get::<bool>("", "untouched").unwrap());
    s.sql(&format!(
        "UPDATE webdock_auth.\"oauthAccessToken\" SET \"sessionId\"='{}'",
        s.session
    ))
    .await;
    let gate = s.f.owner.begin().await.unwrap();
    gate.execute_unprepared("SELECT id FROM webdock_auth.\"oauthAccessToken\" FOR UPDATE")
        .await
        .unwrap();
    let security = s.security.clone();
    let c = s.credentials.clone();
    let token = t.access_token.clone();
    let task = tokio::spawn(async move { security.revoke(&c, &token).await });
    s.wait_lock("oauthAccessToken").await;
    task.abort();
    assert!(matches!(task.await,Err(e)if e.is_cancelled()));
    gate.commit().await.unwrap();
    assert_eq!(s.active(&t.access_token).await["active"], true);
}
#[tokio::test]
#[ignore = "owned PG18; failure after a revocation update rolls the complete security transaction back"]
async fn native_security_late_error_rolls_back_pending_revocation() {
    let s = Setup::start().await;
    let now = chrono::Utc::now().timestamp();
    let jwt=s.sign(&serde_json::json!({"iss":ISSUER,"aud":format!("{ISSUER}/oauth2/userinfo"),"azp":s.credentials.client_id,"sub":s.user,"sid":s.session,"scope":"openid","iat":now,"exp":i64::MAX})).await;
    let hash = legacy_token_hash(&jwt);
    s.sql(&format!("INSERT INTO webdock_auth.\"oauthAccessToken\"(id,token,\"clientId\",\"sessionId\",\"userId\",scopes,resources,\"expiresAt\",\"createdAt\") VALUES('owned-rollback','{hash}','{}','{}','{}','[\"openid\"]','[]',now()+interval '1 hour',now())",s.credentials.client_id,s.session,s.user)).await;
    assert!(matches!(
        s.security.revoke(&s.credentials, &jwt).await,
        Err(Error::InvalidRequest)
    ));
    assert_eq!(
        s.active(&jwt).await["active"],
        true,
        "late invalid timestamp must roll back the earlier UPDATE"
    );
}

impl Setup {
    fn request(&self) -> (AuthorizationRequest, String) {
        let verifier = "n".repeat(64);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut r = AuthorizationRequest::code(
            self.credentials.client_id.clone(),
            REDIRECT.into(),
            "openid profile email offline_access".into(),
            challenge,
        );
        r.state = Some("owned-state".into());
        r.nonce = Some("owned-nonce".into());
        (r, verifier)
    }
    async fn exchange_location(&self, location: &str, verifier: String) -> TokenResponse {
        let url = url::Url::parse(location).unwrap();
        let code = url
            .query_pairs()
            .find(|(k, _)| k == "code")
            .unwrap()
            .1
            .into_owned();
        assert_eq!(
            url.query_pairs().find(|(k, _)| k == "iss").unwrap().1,
            ISSUER
        );
        self.legacy
            .exchange_code(CodeExchange {
                client: self.credentials.clone(),
                code,
                redirect_uri: REDIRECT.into(),
                code_verifier: verifier,
            })
            .await
            .unwrap()
    }
}
#[tokio::test]
#[ignore = "owned PG18; native authorization writes interoperate with existing PKCE code exchange"]
async fn native_grants_create_actual_authorization_codes_without_retained_database() {
    let s = Setup::start().await;
    let (r, verifier) = s.request();
    let AuthorizationOutcome::Redirect(location) =
        s.grants.authorize(&s.auth_cookie, r).await.unwrap()
    else {
        panic!("code redirect expected")
    };
    let token = s.exchange_location(&location, verifier).await;
    assert_eq!(s.active(&token.access_token).await["active"], true);
}

impl Setup {
    async fn pending_consent(&self) -> (ConsentRequest, String) {
        self.sql("UPDATE webdock_auth.\"oauthClient\" SET \"skipConsent\"=false")
            .await;
        let (mut r, verifier) = self.request();
        r.prompt = Some("consent".into());
        let AuthorizationOutcome::ConsentRequired(c) =
            self.grants.authorize(&self.auth_cookie, r).await.unwrap()
        else {
            panic!("consent required")
        };
        (c, verifier)
    }
    async fn consent_counts(&self) -> (i64, i64, i64) {
        let row=self.f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT (SELECT count(*) FROM webdock_auth.\"oauthConsent\") AS consents,(SELECT count(*) FROM webdock_auth.identity_oauth_request WHERE kind='consent') AS pending,(SELECT count(*) FROM webdock_auth.identity_oauth_request WHERE kind='code') AS codes")).await.unwrap().unwrap();
        (
            row.try_get("", "consents").unwrap(),
            row.try_get("", "pending").unwrap(),
            row.try_get("", "codes").unwrap(),
        )
    }
}
#[tokio::test]
#[ignore = "owned PG18; native consent approval is scoped, one-use and interoperable"]
async fn native_grants_consent_approval_and_details_preserve_protocol() {
    let s = Setup::start().await;
    let (c, verifier) = s.pending_consent().await;
    let mut actual = s
        .grants
        .consent_details(&s.auth_cookie, &c.request_id)
        .await
        .unwrap();
    let mut expected = s
        .legacy
        .consent_details(&s.auth_cookie, &c.request_id)
        .await
        .unwrap();
    actual.scopes.sort();
    expected.scopes.sort();
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    let AuthorizationOutcome::Redirect(location) = s
        .grants
        .consent(&s.auth_cookie, &c.request_id, true)
        .await
        .unwrap()
    else {
        panic!("approval code expected")
    };
    assert_eq!(s.consent_counts().await, (1, 0, 1));
    assert!(matches!(
        s.grants.consent(&s.auth_cookie, &c.request_id, true).await,
        Err(Error::InvalidGrant)
    ));
    let token = s.exchange_location(&location, verifier).await;
    assert_eq!(s.active(&token.access_token).await["active"], true);
    let (mut r, _) = s.request();
    r.scope = "openid profile".into();
    assert!(
        matches!(
            s.grants.authorize(&s.auth_cookie, r).await.unwrap(),
            AuthorizationOutcome::Redirect(_)
        ),
        "existing approved consent covers a subset"
    );
}
#[tokio::test]
#[ignore = "owned PG18; native consent denial preserves state/issuer and grants no code"]
async fn native_grants_consent_denial_is_one_use_and_preserves_state() {
    let s = Setup::start().await;
    let (c, _) = s.pending_consent().await;
    let AuthorizationOutcome::Redirect(location) = s
        .grants
        .consent(&s.auth_cookie, &c.request_id, false)
        .await
        .unwrap()
    else {
        panic!("denial redirect expected")
    };
    let url = url::Url::parse(&location).unwrap();
    let p = url
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(p["error"], "access_denied");
    assert_eq!(p["state"], "owned-state");
    assert_eq!(p["iss"], ISSUER);
    assert!(!p.contains_key("code"));
    assert_eq!(s.consent_counts().await, (0, 0, 0));
    assert!(matches!(
        s.grants
            .consent_details(&s.auth_cookie, &c.request_id)
            .await,
        Err(Error::InvalidGrant)
    ));
}
#[tokio::test]
#[ignore = "owned PG18; consent rows and payload are bound to exact verified subject/session/client"]
async fn native_grants_refuse_foreign_session_and_inconsistent_payload() {
    let s = Setup::start().await;
    let (c, _) = s.pending_consent().await;
    s.sql(&format!("INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES('grant-other-session','grant-other-cookie','{}',now()+interval '1 hour',now())",s.user)).await;
    let other = CookiePolicy::new(true, AUTH_SECRET)
        .unwrap()
        .header("grant-other-cookie", 3600)
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    assert!(matches!(
        s.grants.consent(&other, &c.request_id, true).await,
        Err(Error::InvalidGrant)
    ));
    s.sql("UPDATE webdock_auth.identity_oauth_request SET payload=jsonb_set(payload,'{subject}','\"backup\"') WHERE kind='consent'").await;
    assert!(matches!(
        s.grants.consent(&s.auth_cookie, &c.request_id, true).await,
        Err(Error::InvalidGrant)
    ));
    assert_eq!(s.consent_counts().await, (0, 1, 0));
}
#[tokio::test]
#[ignore = "owned PG18; fresh consent expiry and immutable discovery after request-row waits"]
async fn native_grants_recheck_expiry_and_payload_after_consent_lock_wait() {
    let s = Setup::start().await;
    for expire in [true, false] {
        let (c, _) = s.pending_consent().await;
        let hash = legacy_token_hash(&c.request_id);
        let gate = s.f.owner.begin().await.unwrap();
        gate.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT token_hash FROM webdock_auth.identity_oauth_request WHERE token_hash=$1 FOR UPDATE",[hash.clone().into()])).await.unwrap();
        let grants = s.grants.clone();
        let cookie = s.auth_cookie.clone();
        let id = c.request_id.clone();
        let task = tokio::spawn(async move { grants.consent(&cookie, &id, true).await });
        s.wait_lock("identity_oauth_request").await;
        let sql = if expire {
            "UPDATE webdock_auth.identity_oauth_request SET expires_at=clock_timestamp()-interval '1 second' WHERE token_hash=$1"
        } else {
            "UPDATE webdock_auth.identity_oauth_request SET payload=jsonb_set(payload,'{request,nonce}','\"changed\"') WHERE token_hash=$1"
        };
        gate.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            [hash.into()],
        ))
        .await
        .unwrap();
        gate.commit().await.unwrap();
        assert!(matches!(task.await.unwrap(), Err(Error::InvalidGrant)));
        let (consents, _, codes) = s.consent_counts().await;
        assert_eq!(consents, 0);
        assert_eq!(codes, 0);
    }
}
#[tokio::test]
#[ignore = "owned PG18; expired cleanup is bounded and skips locked records"]
async fn native_grants_expired_cleanup_does_not_wait_on_other_requests() {
    let s = Setup::start().await;
    let (r, _) = s.request();
    s.grants.authorize(&s.auth_cookie, r).await.unwrap();
    s.sql("INSERT INTO webdock_auth.identity_oauth_request(token_hash,kind,user_id,session_id,client_id,payload,expires_at) SELECT 'owned-expired-'||g,'code',r.user_id,r.session_id,r.client_id,r.payload,now()-interval '1 hour' FROM (SELECT * FROM webdock_auth.identity_oauth_request LIMIT 1) r CROSS JOIN generate_series(1,120) g").await;
    let gate = s.f.owner.begin().await.unwrap();
    gate.execute_unprepared("SELECT token_hash FROM webdock_auth.identity_oauth_request WHERE token_hash='owned-expired-1' FOR UPDATE").await.unwrap();
    let (r, _) = s.request();
    assert!(matches!(
        tokio::time::timeout(
            Duration::from_secs(2),
            s.grants.authorize(&s.auth_cookie, r)
        )
        .await
        .unwrap()
        .unwrap(),
        AuthorizationOutcome::Redirect(_)
    ));
    let count = s
        .f
        .owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM webdock_auth.identity_oauth_request WHERE expires_at<now()",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", "n")
        .unwrap();
    assert_eq!(count, 20);
    gate.commit().await.unwrap();
}
#[tokio::test]
#[ignore = "owned PG18; parallel approval cannot consume a consent request twice"]
async fn native_grants_parallel_consent_only_issues_one_code() {
    let s = Setup::start().await;
    let (c, _) = s.pending_consent().await;
    let (a, b) = tokio::join!(
        s.grants.consent(&s.auth_cookie, &c.request_id, true),
        s.grants.consent(&s.auth_cookie, &c.request_id, true)
    );
    let r = [a, b];
    assert_eq!(
        r.iter()
            .filter(|r| matches!(r, Ok(AuthorizationOutcome::Redirect(_))))
            .count(),
        1
    );
    assert_eq!(
        r.iter()
            .filter(|r| matches!(r, Err(Error::InvalidGrant)))
            .count(),
        1
    );
    assert_eq!(s.consent_counts().await, (1, 0, 1));
}
#[tokio::test]
#[ignore = "owned PG18; failed code persistence rolls back consent approval and request consumption"]
async fn native_grants_late_write_error_rolls_back_the_whole_decision() {
    let s = Setup::start().await;
    let (c, _) = s.pending_consent().await;
    s.sql("CREATE FUNCTION public.owned_grant_failure() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$ BEGIN IF NEW.kind='code' THEN RAISE EXCEPTION 'owned code failure' USING ERRCODE='40001';END IF;RETURN NEW;END $$;CREATE TRIGGER owned_grant_failure AFTER INSERT ON webdock_auth.identity_oauth_request FOR EACH ROW EXECUTE FUNCTION public.owned_grant_failure()").await;
    assert!(matches!(
        s.grants.consent(&s.auth_cookie, &c.request_id, true).await,
        Err(Error::Configuration)
    ));
    assert_eq!(s.consent_counts().await, (0, 1, 0));
    s.grants
        .consent_details(&s.auth_cookie, &c.request_id)
        .await
        .unwrap();
    s.sql("DROP TRIGGER owned_grant_failure ON webdock_auth.identity_oauth_request")
        .await;
    assert!(matches!(
        s.grants
            .consent(&s.auth_cookie, &c.request_id, true)
            .await
            .unwrap(),
        AuthorizationOutcome::Redirect(_)
    ));
}
#[tokio::test]
#[ignore = "owned PG18; native prompts/max-age/PKCE/redirect checks and preview issuance refusal"]
async fn native_grants_preserve_login_protocol_and_refuse_preview() {
    let s = Setup::start().await;
    let (r, _) = s.request();
    assert!(matches!(
        s.grants.authorize("", r).await.unwrap(),
        AuthorizationOutcome::LoginRequired
    ));
    let (mut r, _) = s.request();
    r.prompt = Some("none".into());
    assert!(matches!(
        s.grants.authorize("", r).await,
        Err(Error::LoginRequired)
    ));
    let (mut r, _) = s.request();
    r.redirect_uri = "https://wrong.example.test/callback".into();
    assert!(s.grants.authorize(&s.auth_cookie, r).await.is_err());
    let (mut r, _) = s.request();
    r.code_challenge = "bad".into();
    assert!(s.grants.authorize(&s.auth_cookie, r).await.is_err());
    let (mut r, _) = s.request();
    r.prompt = Some("login".into());
    assert!(matches!(
        s.grants.authorize(&s.auth_cookie, r).await.unwrap(),
        AuthorizationOutcome::LoginRequired
    ));
    s.sql("UPDATE webdock_auth.session SET \"createdAt\"=now()-interval '1 hour'")
        .await;
    let (mut r, _) = s.request();
    r.max_age = Some(1);
    assert!(matches!(
        s.grants.authorize(&s.auth_cookie, r).await.unwrap(),
        AuthorizationOutcome::LoginRequired
    ));
    s.sql("UPDATE webdock_auth.\"user\" SET role='operator',\"twoFactorEnabled\"=true WHERE id<>'backup'").await;
    s.sql(&format!("INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id) VALUES('{}','{}','100','101')",s.session,s.user)).await;
    let (r, _) = s.request();
    assert!(matches!(
        s.grants.authorize(&s.auth_cookie, r).await,
        Err(Error::AccessDenied)
    ));
    let (_, _, codes) = s.consent_counts().await;
    assert_eq!(codes, 0);
}

#[tokio::test]
#[ignore = "owned PG18; late consent expiry after a pending write cannot issue a code"]
async fn native_grants_recheck_consent_deadline_after_pending_writes() {
    let s = Setup::start().await;
    let (c, _) = s.pending_consent().await;
    s.sql("CREATE FUNCTION public.owned_consent_delay() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$ BEGIN PERFORM pg_advisory_xact_lock(61731233);RETURN NEW;END $$;CREATE TRIGGER owned_consent_delay AFTER INSERT ON webdock_auth.\"oauthConsent\" FOR EACH ROW EXECUTE FUNCTION public.owned_consent_delay();UPDATE webdock_auth.identity_oauth_request SET expires_at=clock_timestamp()+interval '1 second' WHERE kind='consent'").await;
    let gate = s.f.owner.begin().await.unwrap();
    gate.execute_unprepared("SELECT pg_advisory_xact_lock(61731233)")
        .await
        .unwrap();
    let grants = s.grants.clone();
    let cookie = s.auth_cookie.clone();
    let task = tokio::spawn(async move { grants.consent(&cookie, &c.request_id, true).await });
    s.wait_lock("oauthConsent").await;
    gate.execute_unprepared("SELECT pg_sleep(1.1)")
        .await
        .unwrap();
    gate.commit().await.unwrap();
    assert!(
        matches!(task.await.unwrap(), Err(Error::InvalidGrant)),
        "a decision expiring while its pending write waits must roll back"
    );
    assert_eq!(s.consent_counts().await, (0, 1, 0));
}
