//! Owned local PG18; real internal OAuth/PKCE, no external provider credentials/network.
#[path = "../../identity/tests/support/native_pg.rs"]
mod fixture;
use fixture::AUTH_SECRET;
use sea_orm::{ConnectionTrait, Database, DbBackend, Statement, TransactionTrait};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use webdock_browser_session::{BrowserConfig, BrowserSessions};
use webdock_identity::{CookiePolicy, IdentityService, SecretKeys};
use webdock_oauth::{
    AuthorizationOutcome, AuthorizationRequest, ClientAuthMethod, ClientPurpose,
    ClientRegistration, OAuthConfig, OAuthService,
};
use webdock_persistence::{
    IdAllocator,
    postgres::{Limits, Pool},
};
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
            ids,
            keys.clone(),
            config(Some(client.client_id.clone())),
        ));
        let browser = BrowserSessions::new(
            db,
            identity,
            oauth.clone(),
            keys,
            AUTH_SECRET,
            BrowserConfig::new(
                ORIGIN,
                "https://auth.example.test/api/auth",
                client.client_id.clone(),
                client.client_secret.unwrap(),
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
