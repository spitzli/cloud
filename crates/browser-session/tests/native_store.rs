//! Owned local PG18; real internal OAuth/PKCE, no external provider credentials/network.
#[path = "../../identity/tests/support/native_pg.rs"]
mod fixture;
use fixture::AUTH_SECRET;
use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement, TransactionTrait,
};
use spitzli_browser_session::{BrowserConfig, BrowserSessions};
use spitzli_identity::{CookiePolicy, IdentityService, SecretKeys};
use spitzli_oauth::{
    AuthorizationOutcome, AuthorizationRequest, ClientAuthMethod, ClientPurpose,
    ClientRegistration, OAuthConfig, OAuthService,
};
use spitzli_persistence::{
    IdAllocator,
    postgres::{Limits, Pool},
};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
const ORIGIN: &str = "https://console.example.test";
fn cookie(value: &str) -> String {
    value.split(';').next().unwrap().into()
}
fn authorization(location: &str) -> AuthorizationRequest {
    let url = url::Url::parse(location).unwrap();
    let v = url
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    AuthorizationRequest {
        client_id: v["client_id"].to_string(),
        redirect_uri: v["redirect_uri"].to_string(),
        response_type: "code".into(),
        scope: v["scope"].to_string(),
        state: Some(v["state"].to_string()),
        nonce: Some(v["nonce"].to_string()),
        code_challenge: v["code_challenge"].to_string(),
        code_challenge_method: "S256".into(),
        prompt: None,
        max_age: None,
        resources: vec![],
    }
}
struct Setup {
    f: fixture::Fixture,
    browser: Arc<BrowserSessions>,
    oauth: Arc<OAuthService>,
    auth_cookie: String,
    client: String,
    keys: Arc<SecretKeys>,
    ids: Arc<IdAllocator>,
    secret: String,
}
impl Setup {
    async fn start() -> Self {
        Self::start_mode(true).await
    }
    async fn start_mode(native: bool) -> Self {
        let f = fixture::Fixture::start().await;
        let db = Database::connect(&f.bootstrap_url).await.unwrap();
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
        let config = |portal| {
            OAuthConfig::new(
                "https://auth.example.test/api/auth",
                "https://studio.example.test/api/mcp",
                portal,
            )
            .unwrap()
        };
        let bootstrap = OAuthService::new(
            db.clone(),
            identity.clone(),
            ids.clone(),
            keys.clone(),
            config(None),
        );
        bootstrap.provision_signing_key(&f.owner).await.unwrap();
        let client = bootstrap
            .provision_client(
                &f.owner,
                ClientRegistration {
                    name: "Owned browser".into(),
                    redirect_uris: vec![
                        format!("{ORIGIN}/api/sso/callback"),
                        "https://studio.example.test/api/sso/callback".into(),
                    ],
                    post_logout_redirect_uris: vec![format!("{ORIGIN}/login")],
                    scopes: vec!["openid".into(), "profile".into(), "email".into()],
                    auth_method: ClientAuthMethod::ClientSecretPost,
                    allow_refresh: true,
                    skip_consent: true,
                    purpose: ClientPurpose::Portal,
                },
            )
            .await
            .unwrap();
        let oauth = Arc::new(OAuthService::new(
            db.clone(),
            identity.clone(),
            ids.clone(),
            keys.clone(),
            config(Some(client.client_id.clone())),
        ));
        let browser = BrowserSessions::new(
            db,
            identity,
            oauth.clone(),
            keys.clone(),
            AUTH_SECRET,
            BrowserConfig::new(
                ORIGIN,
                "https://auth.example.test/api/auth",
                client.client_id.clone(),
                client.client_secret.clone().unwrap(),
            )
            .unwrap()
            .with_additional_origins(&["https://studio.example.test".into()])
            .unwrap(),
        )
        .unwrap();
        let browser = if native {
            browser.with_native_flow_store(
                Pool::connect(
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
                .unwrap(),
            )
        } else {
            browser
        };
        f.owner.execute_unprepared(r#"INSERT INTO webdock_auth."user"(id,name,email,"emailVerified",role,"twoFactorEnabled","mustChangePassword") VALUES('owned-browser-user','Owned browser','browser@example.test',true,'user',false,false);INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES('owned-browser-session','owned-browser-token','owned-browser-user',now()+interval '1 hour',now());
 WITH hint AS MATERIALIZED(SELECT set_config('webdock.organization_allocation','{"customer_id":"100","organization_id":"101"}',true)) INSERT INTO webdock_admin.customers(id,name) SELECT '100','Owned browser' FROM hint;
 INSERT INTO webdock_auth.member(id,"organizationId","userId",role,"createdAt") VALUES('owned-browser-member','101','owned-browser-user','member',now());
 CREATE TABLE public.owned_browser_audit(kind text,app text);CREATE FUNCTION public.owned_browser_audit() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$ BEGIN INSERT INTO public.owned_browser_audit VALUES(TG_TABLE_NAME,current_setting('application_name'));RETURN NEW;END $$;
 CREATE TRIGGER owned_browser_audit AFTER INSERT ON webdock_auth.identity_browser_flow FOR EACH ROW EXECUTE FUNCTION public.owned_browser_audit()"#).await.unwrap();
        let auth_cookie = cookie(
            &CookiePolicy::new(true, AUTH_SECRET)
                .unwrap()
                .header("owned-browser-token", 3600)
                .unwrap(),
        );
        Self {
            f,
            browser: Arc::new(browser),
            oauth,
            auth_cookie,
            client: client.client_id,
            keys,
            ids,
            secret: client.client_secret.unwrap(),
        }
    }
}
async fn code(s: &Setup, location: &str) -> (String, String) {
    let request = authorization(location);
    let state = request.state.clone().unwrap();
    let AuthorizationOutcome::Redirect(callback) =
        s.oauth.authorize(&s.auth_cookie, request).await.unwrap()
    else {
        panic!("first-party code expected")
    };
    let url = url::Url::parse(&callback).unwrap();
    let code = url
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .into_owned();
    (code, state)
}
#[tokio::test]
#[ignore = "owned PG18; native encrypted browser flow and one-use internal OAuth handoff"]
async fn native_browser_flow_is_encrypted_origin_bound_and_consumed_once() {
    let s = Setup::start().await;
    let begin = s.browser.begin(ORIGIN, "/projects?view=all").await.unwrap();
    let flow_cookie = cookie(&begin.set_cookies[0]);
    let (code, state) = code(&s, &begin.location).await;
    let row =
        s.f.owner
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT payload FROM webdock_auth.identity_browser_flow",
            ))
            .await
            .unwrap()
            .unwrap();
    let encrypted: String = row.try_get("", "payload").unwrap();
    assert!(!encrypted.contains(&state));
    s.f.owner
        .execute_unprepared(
            "UPDATE webdock_auth.identity_browser_flow SET client_id='owned-wrong-client'",
        )
        .await
        .unwrap();
    assert!(
        s.browser
            .callback(ORIGIN, &flow_cookie, &code, &state)
            .await
            .is_err()
    );
    s.f.owner
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_auth.identity_browser_flow SET client_id=$1",
            [s.client.clone().into()],
        ))
        .await
        .unwrap();
    assert!(
        s.browser
            .callback(ORIGIN, &flow_cookie, &code, "wrong-state")
            .await
            .is_err()
    );
    assert!(
        s.browser
            .callback("https://studio.example.test", &flow_cookie, &code, &state)
            .await
            .is_err()
    );
    let browser = s.browser.clone();
    let cookie2 = flow_cookie.clone();
    let code2 = code.clone();
    let state2 = state.clone();
    let other =
        tokio::spawn(async move { browser.callback(ORIGIN, &cookie2, &code2, &state2).await });
    let one = s
        .browser
        .callback(ORIGIN, &flow_cookie, &code, &state)
        .await;
    let two = other.await.unwrap();
    assert_eq!(u8::from(one.is_ok()) + u8::from(two.is_ok()), 1);
    let reply = one.or(two).unwrap();
    assert_eq!(reply.location, format!("{ORIGIN}/projects?view=all"));
    let session_cookie = cookie(&reply.set_cookies[0]);
    assert!(
        s.browser
            .authenticate(ORIGIN, &session_cookie)
            .await
            .unwrap()
            .unwrap()
            .can_access()
    );
    let row=s.f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT count(*) AS n FROM public.owned_browser_audit WHERE app='spitzli-native-postgres'")).await.unwrap().unwrap();
    assert!(
        row.try_get::<i64>("", "n").unwrap() >= 1,
        "actual flow write must use native backend"
    );
    assert!(
        s.browser
            .callback(ORIGIN, &flow_cookie, &code, &state)
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "owned PG18; native flow expiry after row-lock wait cannot pass callback"]
async fn native_browser_flow_cannot_expire_during_lock_wait_and_complete() {
    flow_expiry_after_wait(true).await;
}
#[tokio::test]
#[ignore = "owned PG18; retained flow cannot expire during row-lock wait and complete"]
async fn retained_browser_flow_cannot_expire_during_lock_wait_and_complete() {
    flow_expiry_after_wait(false).await;
}
async fn flow_expiry_after_wait(native: bool) {
    let s = Setup::start_mode(native).await;
    let begin = s.browser.begin(ORIGIN, "/projects").await.unwrap();
    let flow_cookie = cookie(&begin.set_cookies[0]);
    let (code, state) = code(&s, &begin.location).await;
    s.f.owner
        .execute_unprepared(
            "UPDATE webdock_auth.identity_browser_flow SET expires_at=now()+interval '2 seconds'",
        )
        .await
        .unwrap();
    let gate = s.f.owner.begin().await.unwrap();
    gate.execute_unprepared("SELECT token_hash FROM webdock_auth.identity_browser_flow FOR UPDATE")
        .await
        .unwrap();
    let browser = s.browser.clone();
    let task =
        tokio::spawn(async move { browser.callback(ORIGIN, &flow_cookie, &code, &state).await });
    let until = tokio::time::Instant::now() + Duration::from_secs(1);
    loop {
        let blocked=s.f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE usename='webdock_auth_bootstrap' AND wait_event_type='Lock' AND query LIKE '%identity_browser_flow%' AND query LIKE '%FOR UPDATE%') AS blocked")).await.unwrap().unwrap().try_get::<bool>("","blocked").unwrap();
        if blocked {
            break;
        }
        assert!(tokio::time::Instant::now() < until);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(2200)).await;
    gate.commit().await.unwrap();
    assert!(
        task.await.unwrap().is_err(),
        "callback cannot accept flow that expired while waiting"
    );
    let rows=s.f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT (SELECT count(*) FROM webdock_auth.identity_browser_flow) AS flows,(SELECT count(*) FROM webdock_auth.identity_browser_session) AS sessions")).await.unwrap().unwrap();
    assert_eq!(rows.try_get::<i64>("", "flows").unwrap(), 1);
    assert_eq!(rows.try_get::<i64>("", "sessions").unwrap(), 0);
}
#[tokio::test]
#[ignore = "owned PG18; native housekeeping bounded, skip-locked and active-safe"]
async fn native_browser_pruning_keeps_active_and_locked_rows() {
    let s = Setup::start().await;
    for _ in 0..4 {
        s.browser.begin(ORIGIN, "/projects").await.unwrap();
    }
    assert_eq!(s.browser.prune_expired(1).await.unwrap(), 0);
    s.f.owner.execute_unprepared("UPDATE webdock_auth.identity_browser_flow SET expires_at=now()-interval '1 second' WHERE token_hash IN (SELECT token_hash FROM webdock_auth.identity_browser_flow ORDER BY created_at LIMIT 3)").await.unwrap();
    let gate = s.f.owner.begin().await.unwrap();
    gate.execute_unprepared("SELECT token_hash FROM webdock_auth.identity_browser_flow WHERE expires_at<=now() ORDER BY expires_at LIMIT 1 FOR UPDATE").await.unwrap();
    assert_eq!(s.browser.prune_expired(1).await.unwrap(), 1);
    gate.commit().await.unwrap();
    assert_eq!(s.browser.prune_expired(2).await.unwrap(), 2);
    let active = s
        .f
        .owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM webdock_auth.identity_browser_flow WHERE expires_at>now()",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", "n")
        .unwrap();
    assert_eq!(active, 1);
    assert!(s.browser.prune_expired(0).await.is_err());
    assert!(s.browser.prune_expired(10001).await.is_err());
}
#[tokio::test]
#[ignore = "owned PG18; native logout proof remains owner-bound and single-use"]
async fn native_browser_logout_confirmation_is_owner_bound_and_not_replayable() {
    let s = Setup::start().await;
    let begin = s.browser.begin(ORIGIN, "/projects").await.unwrap();
    let (code, state) = code(&s, &begin.location).await;
    let session = s
        .browser
        .callback(ORIGIN, &cookie(&begin.set_cookies[0]), &code, &state)
        .await
        .unwrap();
    let logout = s
        .browser
        .logout(ORIGIN, &cookie(&session.set_cookies[0]))
        .await
        .unwrap();
    let url = url::Url::parse(&logout.location).unwrap();
    let request = url
        .query_pairs()
        .find(|(k, _)| k == "request")
        .unwrap()
        .1
        .into_owned();
    s.f.owner.execute_unprepared(r#"INSERT INTO webdock_auth."user"(id,name,email,"emailVerified",role,"twoFactorEnabled","mustChangePassword") VALUES('owned-other-user','Other','other@example.test',true,'user',false,false);INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES('owned-other-session','owned-other-token','owned-other-user',now()+interval '1 hour',now())"#).await.unwrap();
    let wrong = cookie(
        &CookiePolicy::new(true, AUTH_SECRET)
            .unwrap()
            .header("owned-other-token", 3600)
            .unwrap(),
    );
    assert!(
        s.browser
            .finish_logout(&wrong, &request, true)
            .await
            .is_err()
    );
    assert_eq!(
        s.browser
            .logout_details(&s.auth_cookie, &request)
            .await
            .unwrap()["applicationOrigin"],
        ORIGIN
    );
    let done = s
        .browser
        .finish_logout(&s.auth_cookie, &request, true)
        .await
        .unwrap();
    assert_eq!(done.location, format!("{ORIGIN}/login"));
    assert!(!done.set_cookies.is_empty());
    assert!(
        s.browser
            .finish_logout(&s.auth_cookie, &request, true)
            .await
            .is_err()
    );
    let rows=s.f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT count(*) FILTER(WHERE id='owned-browser-session') AS own,count(*) FILTER(WHERE id='owned-other-session') AS other FROM webdock_auth.session")).await.unwrap().unwrap();
    assert_eq!(rows.try_get::<i64>("", "own").unwrap(), 0);
    assert_eq!(rows.try_get::<i64>("", "other").unwrap(), 1);
}

#[tokio::test]
#[ignore = "owned PG18; retained browser storage remains compatible after domain extraction"]
async fn retained_browser_handoff_and_logout_remain_compatible() {
    let s = Setup::start_mode(false).await;
    let begin = s.browser.begin(ORIGIN, "/projects").await.unwrap();
    let (code, state) = code(&s, &begin.location).await;
    let session = s
        .browser
        .callback(ORIGIN, &cookie(&begin.set_cookies[0]), &code, &state)
        .await
        .unwrap();
    assert!(
        s.browser
            .authenticate(ORIGIN, &cookie(&session.set_cookies[0]))
            .await
            .unwrap()
            .unwrap()
            .can_access()
    );
    let logout = s
        .browser
        .logout(ORIGIN, &cookie(&session.set_cookies[0]))
        .await
        .unwrap();
    let url = url::Url::parse(&logout.location).unwrap();
    let request = url
        .query_pairs()
        .find(|(k, _)| k == "request")
        .unwrap()
        .1
        .into_owned();
    let done = s
        .browser
        .finish_logout(&s.auth_cookie, &request, false)
        .await
        .unwrap();
    assert_eq!(done.location, format!("{ORIGIN}/projects"));
    assert!(done.set_cookies.is_empty());
    assert!(
        s.browser
            .finish_logout(&s.auth_cookie, &request, true)
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "owned PG18; canceled native callback leaves nonce/code usable and releases pool"]
async fn cancelled_native_browser_callback_does_not_consume_flow_or_exhaust_pool() {
    let s = Setup::start().await;
    let begin = s.browser.begin(ORIGIN, "/projects").await.unwrap();
    let flow_cookie = cookie(&begin.set_cookies[0]);
    let (code, state) = code(&s, &begin.location).await;
    let gate = s.f.owner.begin().await.unwrap();
    gate.execute_unprepared("SELECT token_hash FROM webdock_auth.identity_browser_flow FOR UPDATE")
        .await
        .unwrap();
    let browser = s.browser.clone();
    let c = flow_cookie.clone();
    let raw = code.clone();
    let st = state.clone();
    let task = tokio::spawn(async move { browser.callback(ORIGIN, &c, &raw, &st).await });
    let until = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let waiting=s.f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name='spitzli-native-postgres' AND wait_event_type='Lock' AND query LIKE '%identity_browser_flow%' AND query LIKE '%FOR UPDATE%') AS blocked")).await.unwrap().unwrap().try_get::<bool>("","blocked").unwrap();
        if waiting {
            break;
        }
        assert!(tokio::time::Instant::now() < until);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    task.abort();
    assert!(matches!(task.await,Err(error)if error.is_cancelled()));
    gate.rollback().await.unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        s.browser.callback(ORIGIN, &flow_cookie, &code, &state),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result.location, format!("{ORIGIN}/projects"));
}

#[tokio::test]
#[ignore = "owned PG18; expired logout confirmation after lock wait cannot revoke identity"]
async fn logout_proof_expiring_during_lock_wait_is_not_consumed_in_either_backend() {
    for native in [true, false] {
        let s = Setup::start_mode(native).await;
        let begin = s.browser.begin(ORIGIN, "/projects").await.unwrap();
        let (code, state) = code(&s, &begin.location).await;
        let session = s
            .browser
            .callback(ORIGIN, &cookie(&begin.set_cookies[0]), &code, &state)
            .await
            .unwrap();
        let logout = s
            .browser
            .logout(ORIGIN, &cookie(&session.set_cookies[0]))
            .await
            .unwrap();
        let url = url::Url::parse(&logout.location).unwrap();
        let request = url
            .query_pairs()
            .find(|(k, _)| k == "request")
            .unwrap()
            .1
            .into_owned();
        s.f.owner.execute_unprepared("UPDATE webdock_auth.identity_browser_flow SET expires_at=now()+interval '2 seconds'").await.unwrap();
        let gate = s.f.owner.begin().await.unwrap();
        gate.execute_unprepared(
            "SELECT token_hash FROM webdock_auth.identity_browser_flow FOR UPDATE",
        )
        .await
        .unwrap();
        let browser = s.browser.clone();
        let auth = s.auth_cookie.clone();
        let task = tokio::spawn(async move { browser.finish_logout(&auth, &request, true).await });
        let until = tokio::time::Instant::now() + Duration::from_secs(1);
        loop {
            let waiting=s.f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE usename='webdock_auth_bootstrap' AND wait_event_type='Lock' AND query LIKE '%identity_browser_flow%' AND (query LIKE '%DELETE%' OR query LIKE '%FOR UPDATE%')) AS blocked")).await.unwrap().unwrap().try_get::<bool>("","blocked").unwrap();
            if waiting {
                break;
            }
            assert!(tokio::time::Instant::now() < until);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(Duration::from_millis(2200)).await;
        gate.commit().await.unwrap();
        assert!(
            task.await.unwrap().is_err(),
            "logout proof expired during wait must not revoke identity: native={native}"
        );
        let row = s
            .f
            .owner
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT count(*) AS n FROM webdock_auth.session WHERE id='owned-browser-session'",
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.try_get::<i64>("", "n").unwrap(), 1);
    }
}

impl Setup {
    async fn native_reader(&self) -> (BrowserSessions, Pool, Arc<IdentityService>) {
        let pool = Pool::connect(
            &self.f.bootstrap_url,
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
        let identity = Arc::new(IdentityService::new(
            DatabaseConnection::default(),
            CookiePolicy::new(true, AUTH_SECRET).unwrap(),
        ));
        let oauth = Arc::new(
            OAuthService::new(
                DatabaseConnection::default(),
                identity.clone(),
                self.ids.clone(),
                self.keys.clone(),
                OAuthConfig::new(
                    "https://auth.example.test/api/auth",
                    "https://studio.example.test/api/mcp",
                    Some(self.client.clone()),
                )
                .unwrap(),
            )
            .with_native_introspection(pool.clone()),
        );
        let reader = BrowserSessions::new(
            DatabaseConnection::default(),
            identity.clone(),
            oauth,
            self.keys.clone(),
            AUTH_SECRET,
            BrowserConfig::new(
                ORIGIN,
                "https://auth.example.test/api/auth",
                self.client.clone(),
                self.secret.clone(),
            )
            .unwrap()
            .with_additional_origins(&["https://studio.example.test".into()])
            .unwrap(),
        )
        .unwrap()
        .with_native_authority_store(pool.clone());
        (reader, pool, identity)
    }
    async fn signed_cookie(&self) -> String {
        let begin = self.browser.begin(ORIGIN, "/projects").await.unwrap();
        let (code, state) = code(self, &begin.location).await;
        let result = self
            .browser
            .callback(ORIGIN, &cookie(&begin.set_cookies[0]), &code, &state)
            .await
            .unwrap();
        cookie(&result.set_cookies[0])
    }
}
#[tokio::test]
#[ignore = "owned PG18; signed native browser authority with disconnected retained reader"]
async fn native_browser_authority_preserves_real_signed_cookie_and_stored_proof() {
    let s = Setup::start().await;
    let signed = s.signed_cookie().await;
    let (reader, pool, identity) = s.native_reader().await;
    let actual = reader
        .resolve_authority(ORIGIN, &signed)
        .await
        .unwrap()
        .unwrap();
    let expected = s
        .browser
        .resolve_authority(ORIGIN, &signed)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&actual.principal).unwrap(),
        serde_json::to_value(&expected.principal).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&actual.authority).unwrap(),
        serde_json::to_value(&expected.authority).unwrap()
    );
    let checked = reader.revalidate(&actual.authority).await.unwrap();
    assert_eq!(checked.principal.user_id, actual.principal.user_id);
    let mut connection = pool.acquire().await.unwrap();
    let tx = connection.transaction().await.unwrap();
    let (tx, principal) = spitzli_browser_session::revalidate_stored_authority_native_in(
        &identity,
        tx,
        Some(&reader),
        &actual.principal.user_id,
        &actual.principal.session_id,
        &spitzli_browser_session::StoredAuthority::FirstPartyBrowser {
            browser: actual.authority,
        },
    )
    .await
    .unwrap();
    assert_eq!(principal.user_id, actual.principal.user_id);
    tx.commit().await.unwrap();
}

impl Setup {
    async fn sql(&self, sql: &str) {
        self.f.owner.execute_unprepared(sql).await.unwrap();
    }
    async fn wait_authority_lock(&self) {
        let until = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let waiting=self.f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND application_name='spitzli-native-postgres' AND wait_event_type='Lock' AND query LIKE '%identity_browser_session%' AND query LIKE '%FOR SHARE%') AS waiting")).await.unwrap().unwrap().try_get::<bool>("","waiting").unwrap();
            if waiting {
                break;
            }
            assert!(
                tokio::time::Instant::now() < until,
                "native sidecar fence not observed"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    async fn payload(&self) -> String {
        self.f
            .owner
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT payload FROM webdock_auth.identity_browser_session LIMIT 1",
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "payload")
            .unwrap()
    }
    async fn replace_payload(&self, payload: &str) {
        self.f
            .owner
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE webdock_auth.identity_browser_session SET payload=$1",
                [payload.into()],
            ))
            .await
            .unwrap();
    }
}
#[tokio::test]
#[ignore = "owned PG18; cookie/origin/client/token and encrypted role/scope ceilings"]
async fn native_browser_authority_rejects_wrong_proof_and_changed_token_permissions() {
    let s = Setup::start().await;
    let signed = s.signed_cookie().await;
    let (reader, _, _) = s.native_reader().await;
    assert!(
        reader
            .resolve_authority("https://wrong.example.test", &signed)
            .await
            .is_err()
    );
    assert!(
        reader
            .resolve_authority(ORIGIN, &format!("{signed}; {signed}"))
            .await
            .is_err()
    );
    let mut invalid = signed.clone().into_bytes();
    let n = invalid.len();
    invalid[n - 5] = if invalid[n - 5] == b'A' { b'B' } else { b'A' };
    assert!(
        reader
            .resolve_authority(ORIGIN, std::str::from_utf8(&invalid).unwrap())
            .await
            .is_err()
    );
    assert!(
        reader
            .resolve_authority("https://studio.example.test", &signed)
            .await
            .unwrap()
            .is_none()
    );
    let original = s.payload().await;
    let value: serde_json::Value =
        serde_json::from_str(&s.keys.decrypt(&original).unwrap()).unwrap();
    for (field, changed) in [
        ("scopes", serde_json::json!(["openid"])),
        ("role_ceiling", serde_json::Value::Null),
        ("access_token", serde_json::json!("wrong-protected-token")),
    ] {
        let mut invalid = value.clone();
        invalid[field] = changed;
        let encrypted = s.keys.encrypt(&invalid.to_string()).unwrap();
        s.replace_payload(&encrypted).await;
        assert!(
            reader.resolve_authority(ORIGIN, &signed).await.is_err(),
            "{field}"
        );
        s.replace_payload(&original).await;
    }
    s.sql("UPDATE webdock_auth.\"oauthAccessToken\" SET revoked=now()")
        .await;
    assert!(reader.resolve_authority(ORIGIN, &signed).await.is_err());
}
#[tokio::test]
#[ignore = "owned PG18; native caller-owned browser proof retains client/identity/token/sidecar fences"]
async fn native_browser_authority_retains_fences_and_scoped_stored_proof() {
    use spitzli_browser_session::{StoredAuthority, revalidate_stored_authority_native_in};
    let s = Setup::start().await;
    let signed = s.signed_cookie().await;
    let (reader, pool, identity) = s.native_reader().await;
    let actual = reader
        .resolve_authority(ORIGIN, &signed)
        .await
        .unwrap()
        .unwrap();
    let mut connection = pool.acquire().await.unwrap();
    let tx = connection.transaction().await.unwrap();
    let (tx, checked) = reader
        .revalidate_native_in(tx, &actual.authority)
        .await
        .unwrap();
    assert_eq!(checked.principal.session_id, actual.principal.session_id);
    for sql in [
        "SELECT id FROM webdock_auth.\"oauthClient\" FOR UPDATE NOWAIT",
        "SELECT id FROM webdock_auth.\"user\" FOR UPDATE NOWAIT",
        "SELECT id FROM webdock_auth.session FOR UPDATE NOWAIT",
        "SELECT id FROM webdock_auth.\"oauthAccessToken\" FOR UPDATE NOWAIT",
        "SELECT token_hash FROM webdock_auth.identity_browser_session FOR UPDATE NOWAIT",
    ] {
        let gate = s.f.owner.begin().await.unwrap();
        let err = gate
            .query_all_raw(Statement::from_string(DbBackend::Postgres, sql))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("could not obtain lock"), "{err}");
        gate.rollback().await.unwrap();
    }
    tx.commit().await.unwrap();
    let tx = connection.transaction().await.unwrap();
    let (tx, p) = revalidate_stored_authority_native_in(
        &identity,
        tx,
        None,
        &actual.principal.user_id,
        &actual.principal.session_id,
        &StoredAuthority::LocalSession {},
    )
    .await
    .unwrap();
    assert!(p.can_access());
    tx.commit().await.unwrap();
    let tx = connection.transaction().await.unwrap();
    assert!(
        revalidate_stored_authority_native_in(
            &identity,
            tx,
            None,
            "wrong-user",
            &actual.principal.session_id,
            &StoredAuthority::LocalSession {}
        )
        .await
        .is_err()
    );
    drop(connection);
    for change in [0, 1, 2, 3] {
        let mut authority = actual.authority.clone();
        match change {
            0 => authority.scopes.clear(),
            1 => authority.customer_scope = Some("foreign-customer".into()),
            2 => authority.expires_at = chrono::Utc::now().timestamp() - 1,
            _ => authority.client_id = "wrong-client".into(),
        };
        assert!(reader.revalidate(&authority).await.is_err());
    }
}
#[tokio::test]
#[ignore = "owned PG18; immutable encrypted proof and deletion after browser lock wait"]
async fn native_browser_authority_cannot_use_replaced_or_deleted_discovery() {
    let s = Setup::start().await;
    let signed = s.signed_cookie().await;
    let (reader, _, _) = s.native_reader().await;
    let reader = Arc::new(reader);
    let original = s.payload().await;
    let replacement = s.keys.encrypt(&s.keys.decrypt(&original).unwrap()).unwrap();
    let gate = s.f.owner.begin().await.unwrap();
    gate.execute_unprepared(
        "SELECT token_hash FROM webdock_auth.identity_browser_session FOR UPDATE",
    )
    .await
    .unwrap();
    let r = reader.clone();
    let c = signed.clone();
    let task = tokio::spawn(async move { r.resolve_authority(ORIGIN, &c).await });
    s.wait_authority_lock().await;
    gate.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE webdock_auth.identity_browser_session SET payload=$1",
        [replacement.into()],
    ))
    .await
    .unwrap();
    gate.commit().await.unwrap();
    assert!(
        task.await.unwrap().unwrap().is_none(),
        "a changed encrypted proof must not authorize discovered state"
    );
    assert!(
        reader
            .resolve_authority(ORIGIN, &signed)
            .await
            .unwrap()
            .is_some(),
        "a new request may validate the new current proof"
    );
    let gate = s.f.owner.begin().await.unwrap();
    gate.execute_unprepared(
        "SELECT token_hash FROM webdock_auth.identity_browser_session FOR UPDATE",
    )
    .await
    .unwrap();
    let r = reader.clone();
    let task = tokio::spawn(async move { r.resolve_authority(ORIGIN, &signed).await });
    s.wait_authority_lock().await;
    gate.execute_unprepared("DELETE FROM webdock_auth.identity_browser_session")
        .await
        .unwrap();
    gate.commit().await.unwrap();
    assert!(task.await.unwrap().unwrap().is_none());
}
#[tokio::test]
#[ignore = "owned PG18; current browser expiry after row-lock wait in both backends"]
async fn browser_authority_expiry_after_wait_is_denied_in_both_backends() {
    let s = Setup::start().await;
    let signed = s.signed_cookie().await;
    let (reader, _, _) = s.native_reader().await;
    s.sql("UPDATE webdock_auth.identity_browser_session SET expires_at=clock_timestamp()+interval '1 second'").await;
    let gate = s.f.owner.begin().await.unwrap();
    gate.execute_unprepared(
        "SELECT token_hash FROM webdock_auth.identity_browser_session FOR UPDATE",
    )
    .await
    .unwrap();
    let legacy = s.browser.clone();
    let cookie2 = signed.clone();
    let a = tokio::spawn(async move { legacy.resolve_authority(ORIGIN, &cookie2).await });
    let b = tokio::spawn(async move { reader.resolve_authority(ORIGIN, &signed).await });
    s.wait_authority_lock().await;
    gate.execute_unprepared("SELECT pg_sleep(1.1)")
        .await
        .unwrap();
    gate.commit().await.unwrap();
    for result in [a.await.unwrap(), b.await.unwrap()] {
        assert!(
            result.is_err() || result.unwrap().is_none(),
            "expired sidecar cannot be returned as active proof"
        );
    }
}
#[tokio::test]
#[ignore = "owned PG18; cancellation and lock budget cannot retain stale native proof"]
async fn native_browser_authority_cancel_and_timeout_release_connection() {
    let s = Setup::start().await;
    let signed = s.signed_cookie().await;
    let (reader, _, _) = s.native_reader().await;
    let reader = Arc::new(reader);
    let gate = s.f.owner.begin().await.unwrap();
    gate.execute_unprepared(
        "SELECT token_hash FROM webdock_auth.identity_browser_session FOR UPDATE",
    )
    .await
    .unwrap();
    let r = reader.clone();
    let c = signed.clone();
    let task = tokio::spawn(async move { r.resolve_authority(ORIGIN, &c).await });
    s.wait_authority_lock().await;
    task.abort();
    assert!(matches!(task.await,Err(error) if error.is_cancelled()));
    let r = reader.clone();
    let c = signed.clone();
    let task = tokio::spawn(async move { r.resolve_authority(ORIGIN, &c).await });
    s.wait_authority_lock().await;
    assert!(
        tokio::time::timeout(Duration::from_secs(7), task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    gate.commit().await.unwrap();
    assert!(
        reader
            .resolve_authority(ORIGIN, &signed)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
#[ignore = "owned PG18; operator promotion/preview and customer/read-only ceilings"]
async fn native_browser_authority_preserves_preview_and_customer_scope_ceilings() {
    use spitzli_browser_session::{StoredAuthority, revalidate_stored_authority_native_in};
    let s = Setup::start().await;
    let signed = s.signed_cookie().await;
    let (reader, pool, identity) = s.native_reader().await;
    s.sql("UPDATE webdock_auth.\"user\" SET role='operator',\"twoFactorEnabled\"=true WHERE id='owned-browser-user'").await;
    assert!(
        reader.resolve_authority(ORIGIN, &signed).await.is_err(),
        "customer proof cannot be promoted to operator"
    );
    let signed = s.signed_cookie().await;
    s.sql("INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id) VALUES('owned-browser-session','owned-browser-user','100','101')").await;
    let actual = reader
        .resolve_authority(ORIGIN, &signed)
        .await
        .unwrap()
        .unwrap();
    assert!(actual.authority.read_only);
    assert!(!actual.principal.is_operator());
    assert!(actual.authority.customer_scope.is_some());
    assert_eq!(
        actual.authority.customer_scope.as_deref(),
        actual.principal.customer_scope()
    );
    s.sql("UPDATE webdock_auth.studio_tenant_preview SET mode='owner'")
        .await;
    assert!(
        reader.revalidate(&actual.authority).await.is_err(),
        "preview mode change invalidates the old exact scope"
    );
    let mut owner = reader
        .resolve_authority(ORIGIN, &signed)
        .await
        .unwrap()
        .unwrap();
    owner.authority.read_only = true;
    let current = reader.revalidate(&owner.authority).await.unwrap();
    assert!(current.principal.is_acting_customer());
    assert!(
        current.authority.read_only,
        "saved read-only ceiling persists within the same exact live scope"
    );
    let mut connection = pool.acquire().await.unwrap();
    let tx = connection.transaction().await.unwrap();
    assert!(
        revalidate_stored_authority_native_in(
            &identity,
            tx,
            None,
            "owned-browser-user",
            "owned-browser-session",
            &StoredAuthority::LocalSession {}
        )
        .await
        .is_err()
    );
    drop(connection);
    let current_scope = current.principal.customer_scope().unwrap().to_owned();
    for scope in [current_scope.as_str(), "foreign-customer"] {
        let mut connection = pool.acquire().await.unwrap();
        let tx = connection.transaction().await.unwrap();
        let result = revalidate_stored_authority_native_in(
            &identity,
            tx,
            None,
            "owned-browser-user",
            "owned-browser-session",
            &StoredAuthority::CustomerSession {
                customer_scope: scope.into(),
                read_only: true,
            },
        )
        .await;
        if scope == current_scope {
            let (tx, p) = result.unwrap();
            assert_eq!(p.customer_scope(), Some(current_scope.as_str()));
            tx.commit().await.unwrap();
        } else {
            assert!(result.is_err());
        }
    }
}
#[tokio::test]
#[ignore = "owned PG18; bounded retained-cookie bridge uses native reference proof"]
async fn native_browser_authority_keeps_bounded_legacy_cookie_bridge() {
    use spitzli_browser_session::LegacyCookies;
    let s = Setup::start().await;
    let _signed = s.signed_cookie().await;
    let (reader, _, _) = s.native_reader().await;
    let tokens: serde_json::Value =
        serde_json::from_str(&s.keys.decrypt(&s.payload().await).unwrap()).unwrap();
    let old = serde_json::json!({"kind":"session","accessToken":tokens["access_token"],"sub":"owned-browser-user","exp":chrono::Utc::now().timestamp()+300,"idToken":tokens["id_token"]});
    let seal = iron_webcrypto_rs::seal(
        &old,
        &iron_webcrypto_rs::Password::WithId {
            id: "1",
            encryption: AUTH_SECRET.into(),
            integrity: AUTH_SECRET.into(),
        },
        &iron_webcrypto_rs::SealOptions {
            ttl: 300000,
            ..Default::default()
        },
    )
    .unwrap();
    let signed = format!("__Host-webdock-sso={seal}~2");
    let reader = reader.with_legacy(
        LegacyCookies::new(
            std::collections::HashMap::from([("1".into(), AUTH_SECRET.into())]),
            chrono::Utc::now().timestamp() + 300,
        )
        .unwrap(),
    );
    let current = reader
        .resolve_authority(ORIGIN, &signed)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.principal.user_id, "owned-browser-user");
    assert!(current.authority.expires_at <= old["exp"].as_i64().unwrap());
    s.sql("UPDATE webdock_auth.\"oauthAccessToken\" SET revoked=now()")
        .await;
    assert!(reader.resolve_authority(ORIGIN, &signed).await.is_err());
}

#[tokio::test]
#[ignore = "owned PG18; canonical revocation cascade must precede native sidecar lock"]
async fn native_browser_authority_does_not_invert_canonical_revocation_order() {
    let s = Setup::start().await;
    let signed = s.signed_cookie().await;
    let (reader, _, _) = s.native_reader().await;
    let gate = s.f.owner.begin().await.unwrap();
    gate.execute_unprepared(
        "SELECT id FROM webdock_auth.\"user\" WHERE id='owned-browser-user' FOR UPDATE",
    )
    .await
    .unwrap();
    let task = tokio::spawn(async move { reader.resolve_authority(ORIGIN, &signed).await });
    let until = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let waiting=s.f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name='spitzli-native-postgres' AND wait_event_type='Lock' AND query LIKE '%webdock_auth.\"user\"%' AND query LIKE '%FOR SHARE%') AS waiting")).await.unwrap().unwrap().try_get::<bool>("","waiting").unwrap();
        if waiting {
            break;
        }
        assert!(
            tokio::time::Instant::now() < until,
            "native canonical lock not observed"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    gate.execute_unprepared("SET LOCAL lock_timeout='500ms';DELETE FROM webdock_auth.session WHERE id='owned-browser-session'").await.unwrap();
    gate.commit().await.unwrap();
    assert!(task.await.unwrap().is_err());
}
