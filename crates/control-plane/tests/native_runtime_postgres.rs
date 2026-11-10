//! Actual native binary against an exclusively owned PostgreSQL fixture.
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};
use serde_json::{Value, json};
use spitzli_identity::{CookiePolicy, IdentityService};
use spitzli_plugin_host::{AdmissionInput, FileBindings, HostService};
use spitzli_plugin_runtime::PeerIdentity;
use std::{
    process::{Command, Stdio},
    sync::Arc,
    time::Duration,
};
#[allow(
    dead_code,
    reason = "Reuse existing owned PostgreSQL roles and migration fixture"
)]
#[path = "../../plugin-host/tests/support/jobs_fixture.rs"]
mod fixture;
fn peer() -> PeerIdentity {
    PeerIdentity::Unix {
        uid: rustix::process::geteuid().as_raw(),
    }
}
async fn sql(db: &impl ConnectionTrait, text: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        text,
        values,
    ))
    .await
    .unwrap();
}
struct Process(std::process::Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
async fn journal(db: &impl ConnectionTrait) -> Value {
    db.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT coalesce(jsonb_agg(to_jsonb(j) ORDER BY version),'[]') AS journal FROM webdock_api_migrations.rust_journal j")).await.unwrap().unwrap().try_get("","journal").unwrap()
}

#[tokio::test]
#[ignore = "owned local PostgreSQL18; actual binary startup and SIGTERM"]
async fn actual_native_binary_starts_with_restricted_roles_without_schema_changes() {
    actual_core(NativeMode::Legacy).await;
}

#[tokio::test]
#[ignore = "owned local PostgreSQL18; actual HTTP write using native ID driver"]
async fn actual_http_write_uses_native_postgres_id_backend() {
    actual_core(NativeMode::Ids).await;
}

#[tokio::test]
#[ignore = "owned local PostgreSQL18; actual authenticated HTTP native session read"]
async fn actual_http_session_read_uses_native_postgres() {
    actual_core(NativeMode::SessionReads).await;
}

#[tokio::test]
#[ignore = "owned local PostgreSQL18; actual native HTTP session listing/revocation/logout"]
async fn actual_http_session_management_uses_native_postgres() {
    actual_core(NativeMode::SessionManagement).await;
}
#[tokio::test]
#[ignore = "owned local PostgreSQL18; actual HTTP first-party login persists native nonce"]
async fn actual_http_browser_login_begin_uses_native_flow_storage() {
    actual_core(NativeMode::BrowserFlows).await;
}
#[tokio::test]
#[ignore = "owned local PostgreSQL18; actual HTTP JWKS uses checked native public-key read"]
async fn actual_http_jwks_uses_native_oauth_client_store() {
    actual_core(NativeMode::OAuthClientReads).await;
}
#[tokio::test]
#[ignore = "owned local PostgreSQL18; actual HTTP native token introspection/userinfo"]
async fn actual_http_token_introspection_uses_native_postgres() {
    actual_core(NativeMode::OAuthIntrospection).await;
}
#[tokio::test]
#[ignore = "owned PostgreSQL18; actual native browser OAuth handoff and cookie session"]
async fn actual_http_browser_authority_uses_native_postgres() {
    actual_core(NativeMode::BrowserAuthority).await;
}
#[tokio::test]
#[ignore = "owned PostgreSQL18; actual native OAuth security write"]
async fn actual_http_oauth_security_uses_native_postgres() {
    actual_core(NativeMode::OAuthSecurity).await;
}
#[tokio::test]
#[ignore = "owned PostgreSQL18; actual native grant creation with PKCE browser handoff"]
async fn actual_http_oauth_grants_use_native_postgres() {
    actual_core(NativeMode::OAuthGrants).await;
}
#[tokio::test]
#[ignore = "owned PostgreSQL18; full native OAuth exchange/signature/refresh HTTP flow"]
async fn actual_http_oauth_tokens_use_native_postgres() {
    actual_core(NativeMode::OAuthTokens).await;
}
#[tokio::test]
#[ignore = "owned PostgreSQL18; actual bearer resource boundary uses native MCP reference proof"]
async fn actual_http_mcp_authority_uses_native_postgres() {
    actual_core(NativeMode::McpAuthority).await;
}
#[derive(Clone, Copy)]
enum NativeMode {
    PasswordLogin,
    AccountRecovery,
    Legacy,
    Ids,
    SessionReads,
    SessionManagement,
    BrowserFlows,
    BrowserAuthority,
    OAuthClientReads,
    OAuthIntrospection,
    OAuthSecurity,
    OAuthGrants,
    OAuthTokens,
    McpAuthority,
}
#[tokio::test]
#[ignore = "owned PostgreSQL18; actual Console recovery HTTP uses native SQL and one-use proof"]
async fn actual_http_account_recovery_uses_native_postgres() {
    actual_core(NativeMode::AccountRecovery).await;
}
#[tokio::test]
#[ignore = "owned PostgreSQL18; actual native recovery/password/MFA/session HTTP flow"]
async fn actual_http_password_and_mfa_login_use_native_postgres() {
    actual_core(NativeMode::PasswordLogin).await;
}
async fn actual_core(mode: NativeMode) {
    let native_password = matches!(mode, NativeMode::PasswordLogin);
    let native_recovery = matches!(mode, NativeMode::AccountRecovery) || native_password;
    let native_mcp = matches!(mode, NativeMode::McpAuthority);
    let native = matches!(mode, NativeMode::Ids);
    let native_session = matches!(mode, NativeMode::SessionReads);
    let native_management = matches!(mode, NativeMode::SessionManagement);
    let native_tokens = matches!(mode, NativeMode::OAuthTokens);
    let native_grants = matches!(mode, NativeMode::OAuthGrants) || native_tokens;
    let native_authority = matches!(mode, NativeMode::BrowserAuthority) || native_grants;
    let native_browser = matches!(mode, NativeMode::BrowserFlows) || native_authority;
    let native_oauth = matches!(mode, NativeMode::OAuthClientReads);
    let native_security = matches!(mode, NativeMode::OAuthSecurity);
    let native_introspection = matches!(mode, NativeMode::OAuthIntrospection) || native_security;
    let fixture = fixture::DatabaseFixture::start().await;
    let owner = fixture.owner().await;
    if native_recovery {
        let hash = spitzli_identity::hash_password("Owned-original-password-123").unwrap();
        owner.execute_unprepared(r#"INSERT INTO webdock_auth."user"(id,name,email,"emailVerified",role,"twoFactorEnabled","mustChangePassword") VALUES('owned-recovery-user','Owned recovery','recovery@example.test',true,'user',true,false);CREATE TABLE public.owned_recovery_audit(app text);CREATE FUNCTION public.owned_recovery_audit() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$ BEGIN INSERT INTO public.owned_recovery_audit VALUES(current_setting('application_name'));RETURN NEW;END $$;CREATE TRIGGER owned_recovery_audit AFTER INSERT ON webdock_auth.identity_recovery_token FOR EACH ROW EXECUTE FUNCTION public.owned_recovery_audit()"#).await.unwrap();
        sql(&owner,r#"INSERT INTO webdock_auth.account(id,"accountId","providerId","userId",password,"updatedAt") VALUES('owned-recovery-credential','owned-recovery-user','credential','owned-recovery-user',$1,now())"#,vec![hash.into()]).await;
    }
    if native_password {
        owner.execute_unprepared(r#"WITH hint AS MATERIALIZED(SELECT set_config('webdock.organization_allocation','{"customer_id":"61001","organization_id":"61002"}',true)) INSERT INTO webdock_admin.customers(id,name) SELECT '61001','Owned login customer' FROM hint;
        WITH hint AS MATERIALIZED(SELECT set_config('webdock.organization_allocation','{"customer_id":"62001","organization_id":"62002"}',true)) INSERT INTO webdock_admin.customers(id,name) SELECT '62001','Foreign login customer' FROM hint;
        INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES('61003','Owned project','61001'),('62003','Foreign project','62001');
        INSERT INTO webdock_auth.member(id,"organizationId","userId",role,"createdAt") VALUES('61004','61002','owned-recovery-user','owner',now())"#).await.unwrap();
        let keys = spitzli_identity::SecretKeys::new(
            std::collections::BTreeMap::from([(
                1,
                "owned-native-http-secret-at-least-32bytes".into(),
            )]),
            1,
            None,
        )
        .unwrap();
        sql(&owner,r#"INSERT INTO webdock_auth."twoFactor"(id,secret,"backupCodes","userId",verified) VALUES('owned-password-factor',$1,$2,'owned-recovery-user',true)"#,vec![keys.encrypt("0123456789abcdefghijklmnopqrstuv").unwrap().into(),keys.encrypt("[\"owned-http-recovery-code\"]").unwrap().into()]).await;
        owner.execute_unprepared("CREATE TABLE public.owned_login_audit(app text);CREATE FUNCTION public.owned_login_audit() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$ BEGIN INSERT INTO public.owned_login_audit VALUES(current_setting('application_name'));RETURN NEW;END $$;CREATE TRIGGER owned_login_audit AFTER INSERT ON webdock_auth.session FOR EACH ROW EXECUTE FUNCTION public.owned_login_audit()").await.unwrap();
    }
    if native_session || native_management {
        owner.execute_unprepared(r#"INSERT INTO webdock_auth."user"(id,name,email,"emailVerified",role,"twoFactorEnabled","mustChangePassword") VALUES('owned-session-user','Owned user','session@example.test',true,'user',false,false); INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES('owned-session','owned-core-session-token','owned-session-user',now()+interval '1 hour',now())"#).await.unwrap();
    }
    if native_management {
        owner.execute_unprepared(r#"INSERT INTO webdock_auth."user"(id,name,email,"emailVerified",role,"twoFactorEnabled","mustChangePassword") VALUES('foreign-user','Foreign','foreign@example.test',true,'user',false,false);
        INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES('owned-other-session','owned-other-token','owned-session-user',now()+interval '1 hour',now()),('foreign-session','foreign-token','foreign-user',now()+interval '1 hour',now())"#).await.unwrap();
    }
    if native_browser || native_introspection {
        let client_hash = spitzli_oauth::legacy_token_hash("owned-core-browser-secret");
        sql(&owner,r#"INSERT INTO webdock_auth."oauthClient"(id,"clientId","clientSecret",name,disabled,"skipConsent","enableEndSession",scopes,"redirectUris","postLogoutRedirectUris","grantTypes","responseTypes","tokenEndpointAuthMethod","requirePKCE","subjectType",metadata,"createdAt","updatedAt") VALUES('103','102',$1,'Owned browser',false,true,true,'["openid","profile","email"]','["http://localhost:3121/api/sso/callback","http://localhost:3120/api/sso/callback"]','["http://localhost:3121/login"]','["authorization_code","refresh_token"]','["code"]','client_secret_post',true,'public','{"webdock_binding":"104"}',now(),now())"#,vec![client_hash.into()]).await;
        owner.execute_unprepared("INSERT INTO webdock_auth.app_binding(id,client_id,label,organization_id,enabled) VALUES('104','102','Owned browser',null,true);CREATE TABLE public.owned_browser_http_audit(app text);CREATE FUNCTION public.owned_browser_http_audit() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$ BEGIN INSERT INTO public.owned_browser_http_audit VALUES(current_setting('application_name'));RETURN NEW;END $$;CREATE TRIGGER owned_browser_http_audit AFTER INSERT ON webdock_auth.identity_browser_flow FOR EACH ROW EXECUTE FUNCTION public.owned_browser_http_audit()").await.unwrap();
    }
    if native_oauth || native_authority {
        let keys = spitzli_identity::SecretKeys::new(
            std::collections::BTreeMap::from([(
                1,
                "owned-native-http-secret-at-least-32bytes".into(),
            )]),
            1,
            None,
        )
        .unwrap();
        let tx = owner.begin().await.unwrap();
        spitzli_oauth::stage_signing_key(&tx, "301", &keys)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    if native_introspection || native_authority || native_mcp {
        owner.execute_unprepared(r#"INSERT INTO webdock_auth."user"(id,name,email,"emailVerified",role,"twoFactorEnabled","mustChangePassword") VALUES('501','HTTP user','http@example.test',true,'user',false,false);INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES('502','owned-introspection-session','501',now()+interval '1 hour',now());
        WITH hint AS MATERIALIZED(SELECT set_config('webdock.organization_allocation','{"customer_id":"100","organization_id":"101"}',true)) INSERT INTO webdock_admin.customers(id,name) SELECT '100','Owned HTTP customer' FROM hint;
        INSERT INTO webdock_auth.member(id,"organizationId","userId",role,"createdAt") VALUES('503','101','501','owner',now())"#).await.unwrap();
        if !native_mcp {
            sql(&owner,r#"INSERT INTO webdock_auth."oauthAccessToken"(id,token,"clientId","sessionId","userId",scopes,resources,"expiresAt","createdAt") VALUES('504',$1,'102','502','501','["openid","profile","email"]','[]',now()+interval '1 hour',now())"#,vec![spitzli_oauth::legacy_token_hash("owned-http-access").into()]).await;
        }
    }
    if native_grants {
        owner.execute_unprepared("CREATE TABLE public.owned_oauth_grant_audit(app text);CREATE FUNCTION public.owned_oauth_grant_audit() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$ BEGIN INSERT INTO public.owned_oauth_grant_audit VALUES(current_setting('application_name'));RETURN NEW;END $$;CREATE TRIGGER owned_oauth_grant_audit AFTER INSERT ON webdock_auth.identity_oauth_request FOR EACH ROW WHEN(NEW.kind='code') EXECUTE FUNCTION public.owned_oauth_grant_audit()").await.unwrap();
    }
    if native_tokens {
        owner.execute_unprepared("UPDATE webdock_auth.\"oauthClient\" SET scopes='[\"openid\",\"profile\",\"email\",\"offline_access\"]' WHERE \"clientId\"='102';CREATE TABLE public.owned_oauth_token_audit(app text,kind text);CREATE FUNCTION public.owned_oauth_token_audit() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$ BEGIN INSERT INTO public.owned_oauth_token_audit VALUES(current_setting('application_name'),TG_TABLE_NAME);RETURN NEW;END $$;CREATE TRIGGER owned_oauth_token_audit AFTER INSERT ON webdock_auth.\"oauthAccessToken\" FOR EACH ROW EXECUTE FUNCTION public.owned_oauth_token_audit();CREATE TRIGGER owned_oauth_token_audit AFTER INSERT ON webdock_auth.\"oauthRefreshToken\" FOR EACH ROW EXECUTE FUNCTION public.owned_oauth_token_audit()").await.unwrap();
    }
    if native_mcp {
        sql(&owner,r#"INSERT INTO webdock_auth."oauthClient"(id,"clientId","clientSecret",name,disabled,"skipConsent","enableEndSession",scopes,"redirectUris","grantTypes","responseTypes","tokenEndpointAuthMethod","requirePKCE","subjectType",metadata,"createdAt","updatedAt") VALUES('901','902',$1,'Owned MCP',false,true,true,'["hosting:read","hosting:write"]','["http://localhost:3121/api/sso/callback"]','["authorization_code"]','["code"]','client_secret_post',true,'public','{"webdock_hosting":true}',now(),now())"#,vec![spitzli_oauth::legacy_token_hash("owned-http-mcp-secret").into()]).await;
        owner.execute_unprepared(r#"INSERT INTO webdock_auth."oauthResource"(id,identifier,name,"accessTokenTtl","refreshTokenTtl","allowedScopes",disabled,"createdAt","updatedAt") VALUES('904','http://localhost:3120/api/mcp','Owned MCP',300,28800,'["hosting:read","hosting:write"]',false,now(),now());INSERT INTO webdock_auth."oauthClientResource"(id,"clientId","resourceId","createdAt") VALUES('905','902','http://localhost:3120/api/mcp',now())"#).await.unwrap();
        sql(&owner,r#"INSERT INTO webdock_auth."oauthAccessToken"(id,token,"clientId","sessionId","userId",scopes,resources,"expiresAt","createdAt") VALUES('903',$1,'902','502','501','["hosting:read"]','["http://localhost:3120/api/mcp"]',now()+interval '300 seconds',now())"#,vec![spitzli_oauth::legacy_token_hash("owned-http-mcp").into()]).await;
    }
    let before = journal(&owner).await;
    let account_counts = || {
        Statement::from_string(
            DbBackend::Postgres,
            "SELECT jsonb_build_object('users',(SELECT count(*) FROM webdock_auth.\"user\"),'customers',(SELECT count(*) FROM webdock_admin.customers),'projects',(SELECT count(*) FROM webdock_admin.projects)) AS counts",
        )
    };
    let before_accounts: Value = owner
        .query_one_raw(account_counts())
        .await
        .unwrap()
        .unwrap()
        .try_get("", "counts")
        .unwrap();
    let password = uuid::Uuid::new_v4().simple().to_string();
    let mut urls = std::collections::BTreeMap::new();
    for role in [
        "webdock_auth_bootstrap",
        "webdock_auth_runtime",
        "webdock_worker_runtime",
        "webdock_registry_runtime",
    ] {
        owner
            .execute_unprepared(&format!("ALTER ROLE {role} PASSWORD '{password}'"))
            .await
            .unwrap();
        let mut url = fixture.url.clone();
        url.set_username(role).unwrap();
        url.set_password(Some(&password)).unwrap();
        urls.insert(role, url.to_string());
    }
    let reserve = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = reserve.local_addr().unwrap().port();
    drop(reserve);
    let private = tempfile::tempdir().unwrap();
    let log = private.path().join("core.log");
    let secret = "owned-native-http-secret-at-least-32bytes";
    let mut command = Command::new(env!("CARGO_BIN_EXE_spitzli-control-plane"));
    command
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .env("HOME", private.path())
        .env("NODE_ENV", "development")
        .env("PORT", port.to_string())
        .env("WEBDOCK_BIND_ADDRESS", "127.0.0.1")
        .env("WEBDOCK_DATABASE_URL", &urls["webdock_auth_bootstrap"])
        .env(
            "WEBDOCK_SESSION_DATABASE_URL",
            &urls["webdock_auth_runtime"],
        )
        .env(
            "WEBDOCK_WORKER_DATABASE_URL",
            &urls["webdock_worker_runtime"],
        )
        .env(
            "WEBDOCK_REGISTRY_DATABASE_URL",
            &urls["webdock_registry_runtime"],
        )
        .env("WEBDOCK_AUTH_SECRET", secret)
        .env(
            "DATABASE_RLS_SECRET",
            "owned-plugin-jobs-rls-secret-32bytes",
        )
        .env("WEBDOCK_API_ORIGIN", format!("http://127.0.0.1:{port}"))
        .env("WEBDOCK_AUTH_ORIGIN", "http://localhost:3125")
        .env("WEBDOCK_STUDIO_ORIGIN", "http://localhost:3120")
        .env("WEBDOCK_CONSOLE_ORIGIN", "http://localhost:3121")
        .env("WEBDOCK_FEATURES", "{}")
        .env(
            "WEBDOCK_ID_BACKEND",
            if native { "tokio-postgres" } else { "sea-orm" },
        )
        .env(
            "WEBDOCK_SESSION_READ_BACKEND",
            if native_session {
                "tokio-postgres"
            } else {
                "sea-orm"
            },
        );
    command.env(
        "WEBDOCK_SESSION_MANAGEMENT_BACKEND",
        if native_management {
            "tokio-postgres"
        } else {
            "sea-orm"
        },
    );
    command.env(
        "WEBDOCK_BROWSER_FLOW_BACKEND",
        if native_browser && !native_authority {
            "tokio-postgres"
        } else {
            "sea-orm"
        },
    );
    if native_browser {
        command
            .env("WEBDOCK_SSO_CLIENT_ID", "102")
            .env("WEBDOCK_SSO_CLIENT_SECRET", "owned-core-browser-secret")
            .env("WEBDOCK_SSO_COOKIE_SECRET", secret);
    }
    command.env(
        "WEBDOCK_OAUTH_CLIENT_BACKEND",
        if (native_oauth || native_browser) && !native_authority {
            "tokio-postgres"
        } else {
            "sea-orm"
        },
    );
    command.env(
        "WEBDOCK_OAUTH_INTROSPECTION_BACKEND",
        if native_introspection && !native_security {
            "tokio-postgres"
        } else {
            "sea-orm"
        },
    );
    if native_introspection {
        command.env("WEBDOCK_STUDIO_CLIENT_ID", "102");
    }
    command.env(
        "WEBDOCK_BROWSER_AUTHORITY_BACKEND",
        if native_authority {
            "tokio-postgres"
        } else {
            "sea-orm"
        },
    );
    command.env(
        "WEBDOCK_OAUTH_SECURITY_BACKEND",
        if native_security {
            "tokio-postgres"
        } else {
            "sea-orm"
        },
    );
    command.env(
        "WEBDOCK_OAUTH_GRANTS_BACKEND",
        if native_grants && !native_tokens {
            "tokio-postgres"
        } else {
            "sea-orm"
        },
    );
    command.env(
        "WEBDOCK_OAUTH_TOKENS_BACKEND",
        if native_tokens {
            "tokio-postgres"
        } else {
            "sea-orm"
        },
    );
    command.env(
        "WEBDOCK_MCP_AUTHORITY_BACKEND",
        if native_mcp {
            "tokio-postgres"
        } else {
            "sea-orm"
        },
    );
    command.env(
        "WEBDOCK_ACCOUNT_RECOVERY_BACKEND",
        if native_recovery {
            "tokio-postgres"
        } else {
            "sea-orm"
        },
    );
    command.env(
        "WEBDOCK_PASSWORD_LOGIN_BACKEND",
        if native_password {
            "tokio-postgres"
        } else {
            "sea-orm"
        },
    );
    if native_mcp {
        use std::os::unix::fs::PermissionsExt;
        let path = private.path().join("resource-secret");
        std::fs::write(&path, "owned-http-mcp-secret").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        command
            .env("WEBDOCK_RESOURCE_CLIENT_ID", "902")
            .env("WEBDOCK_RESOURCE_CLIENT_SECRET_FILE", path);
    }
    let output = std::fs::File::create(&log).unwrap();
    command
        .stdout(Stdio::from(output.try_clone().unwrap()))
        .stderr(Stdio::from(output));
    let mut process = Process(command.spawn().unwrap());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        assert!(
            process.0.try_wait().unwrap().is_none(),
            "actual Core exited before readiness; owned log {}",
            log.display()
        );
        if let Ok(response) = client
            .get(format!("http://127.0.0.1:{port}/readyz"))
            .send()
            .await
            && response.status() == 200
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "actual Core readiness timeout"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        client
            .get(format!("http://127.0.0.1:{port}/healthz"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    if native {
        let response=client.post(format!("http://127.0.0.1:{port}/api/console/waitlist")).header("Host","localhost:3120").header("Origin","http://localhost:3120")
            .json(&json!({"email":"native-id@example.test","name":"Owned native ID test","privacyAccepted":true})).send().await.unwrap();
        assert_eq!(response.status(), 202);
        let row=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT (SELECT count(*) FROM webdock_auth.waitlist_entry WHERE email='native-id@example.test') AS entries,(SELECT count(*) FROM pg_catalog.pg_stat_activity WHERE application_name='spitzli-native-postgres' AND usename='webdock_auth_bootstrap') AS native_connections")).await.unwrap().unwrap();
        assert_eq!(row.try_get::<i64>("", "entries").unwrap(), 1);
        assert!(row.try_get::<i64>("", "native_connections").unwrap() >= 1);
    }
    if native_recovery {
        let url = format!("http://127.0.0.1:{port}/api/console/auth/request-password-reset");
        let response=client.post(&url).header("Host","localhost:3121").header("Origin","http://localhost:3121").json(&json!({"email":"recovery@example.test","redirectTo":"https://foreign.invalid/steal"})).send().await.unwrap();
        assert_eq!(response.status(), 200);
        let expected: Value = response.json().await.unwrap();
        let missing: Value = client
            .post(&url)
            .header("Host", "localhost:3121")
            .header("Origin", "http://localhost:3121")
            .json(&json!({"email":"missing@example.test"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(expected, missing);
        let row = owner
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT app FROM public.owned_recovery_audit",
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            row.try_get::<String>("", "app").unwrap(),
            "spitzli-native-postgres"
        );
        let keys = spitzli_identity::SecretKeys::new(
            std::collections::BTreeMap::from([(1, secret.to_owned())]),
            1,
            Some(secret.to_owned()),
        )
        .unwrap();
        let row = owner
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT payload FROM webdock_auth.identity_delivery_job",
            ))
            .await
            .unwrap()
            .unwrap();
        let request: spitzli_identity::DeliveryRequest = serde_json::from_str(
            &keys
                .decrypt(&row.try_get::<String>("", "payload").unwrap())
                .unwrap(),
        )
        .unwrap();
        assert!(
            request
                .url
                .starts_with("http://localhost:3125/reset-password?token=")
        );
        let token = url::Url::parse(&request.url)
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "token")
            .unwrap()
            .1
            .into_owned();
        let reset = format!("http://127.0.0.1:{port}/api/console/auth/reset-password");
        let response = client
            .post(&reset)
            .timeout(Duration::from_secs(15))
            .header("Host", "localhost:3121")
            .header("Origin", "http://localhost:3121")
            .json(&json!({"token":token,"newPassword":"Owned-new-password-456"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let replay = client
            .post(&reset)
            .header("Host", "localhost:3121")
            .header("Origin", "http://localhost:3121")
            .json(&json!({"token":token,"newPassword":"Owned-other-password-789"}))
            .send()
            .await
            .unwrap();
        assert_eq!(replay.status(), 401);
        let signin = client
            .post(format!(
                "http://127.0.0.1:{port}/api/console/auth/sign-in/email"
            ))
            .timeout(Duration::from_secs(15))
            .header("Host", "localhost:3121")
            .header("Origin", "http://localhost:3121")
            .json(&json!({"email":"recovery@example.test","password":"Owned-new-password-456"}))
            .send()
            .await
            .unwrap();
        assert_eq!(signin.status(), 200);
        let mfa_cookie = signin
            .headers()
            .get_all("set-cookie")
            .iter()
            .map(|h| h.to_str().unwrap())
            .find(|h| h.starts_with("webdock-auth.two_factor="))
            .map(|h| h.split(';').next().unwrap().to_owned());
        assert!(
            signin
                .headers()
                .get_all("set-cookie")
                .iter()
                .all(|h| !h.to_str().unwrap().contains("session_token=")),
            "password reset must not bypass the retained MFA gate"
        );
        let body: Value = signin.json().await.unwrap();
        assert_eq!(body["twoFactorRedirect"], true);
        if native_password {
            let mfa_cookie = mfa_cookie.unwrap();
            let endpoint =
                format!("http://127.0.0.1:{port}/api/console/auth/two-factor/verify-backup-code");
            let verified = client
                .post(&endpoint)
                .header("Host", "localhost:3121")
                .header("Origin", "http://localhost:3121")
                .header("Cookie", &mfa_cookie)
                .json(&json!({"code":"owned-http-recovery-code"}))
                .send()
                .await
                .unwrap();
            assert_eq!(verified.status(), 200);
            let session_cookie = verified
                .headers()
                .get_all("set-cookie")
                .iter()
                .map(|h| h.to_str().unwrap())
                .find(|h| h.contains("session_token=") && !h.contains("Max-Age=0"))
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .to_owned();
            let session = client
                .get(format!("http://127.0.0.1:{port}/v1/auth/session"))
                .header("Cookie", &session_cookie)
                .send()
                .await
                .unwrap();
            assert_eq!(session.status(), 200);
            let body: Value = session.json().await.unwrap();
            assert_eq!(body["data"]["user"]["id"], "owned-recovery-user");
            assert_eq!(body["data"]["user"]["role"], "user");
            let project = client
                .get(format!("http://127.0.0.1:{port}/api/console/project/61003"))
                .header("Host", "localhost:3121")
                .header("Cookie", &session_cookie)
                .send()
                .await
                .unwrap();
            let status = project.status();
            let text = project.text().await.unwrap();
            assert_eq!(status, 200, "owned project response: {text}");
            let project: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(project["id"], "61003");
            assert_eq!(project["customer"]["id"], "61001");
            let foreign = client
                .get(format!("http://127.0.0.1:{port}/api/console/project/62003"))
                .header("Host", "localhost:3121")
                .header("Cookie", &session_cookie)
                .send()
                .await
                .unwrap();
            assert_eq!(foreign.status(), 404);
            let audit = owner
                .query_one_raw(Statement::from_string(
                    DbBackend::Postgres,
                    "SELECT app FROM public.owned_login_audit",
                ))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                audit.try_get::<String>("", "app").unwrap(),
                "spitzli-native-postgres"
            );
            let replay = client
                .post(&endpoint)
                .header("Host", "localhost:3121")
                .header("Origin", "http://localhost:3121")
                .header("Cookie", mfa_cookie)
                .json(&json!({"code":"owned-http-recovery-code"}))
                .send()
                .await
                .unwrap();
            assert_eq!(replay.status(), 401);
        }
    }
    if native_session {
        let gate = owner.begin().await.unwrap();
        gate.execute_unprepared("LOCK TABLE webdock_auth.session IN ACCESS EXCLUSIVE MODE")
            .await
            .unwrap();
        let cookie = CookiePolicy::new(false, secret)
            .unwrap()
            .header("owned-core-session-token", 3600)
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let request_client = client.clone();
        let request = tokio::spawn(async move {
            request_client
                .get(format!("http://127.0.0.1:{port}/api/auth/get-session"))
                .header("Host", "localhost:3125")
                .header("Cookie", cookie)
                .timeout(Duration::from_secs(5))
                .send()
                .await
                .unwrap()
        });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let active=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_catalog.pg_stat_activity WHERE application_name='spitzli-native-postgres' AND usename='webdock_auth_bootstrap' AND wait_event_type='Lock' AND query LIKE '%s.token=$1%' AND query LIKE '%native_provider_session_live%') AS native_read")).await.unwrap().unwrap().try_get::<bool>("","native_read").unwrap();
            if active {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "actual authenticated HTTP must execute the native session query"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        gate.commit().await.unwrap();
        let response = request.await.unwrap();
        assert_eq!(response.status(), 200);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["user"]["id"], "owned-session-user");
    }
    if native_management {
        let gate = owner.begin().await.unwrap();
        gate.execute_unprepared("LOCK TABLE webdock_auth.session IN ACCESS EXCLUSIVE MODE")
            .await
            .unwrap();
        let auth_cookie = CookiePolicy::new(false, secret)
            .unwrap()
            .header("owned-core-session-token", 3600)
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let request_client = client.clone();
        let request_cookie = auth_cookie.clone();
        let request = tokio::spawn(async move {
            request_client
                .get(format!("http://127.0.0.1:{port}/api/auth/list-sessions"))
                .header("Host", "localhost:3125")
                .header("Cookie", request_cookie)
                .timeout(Duration::from_secs(5))
                .send()
                .await
                .unwrap()
        });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let active=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name='spitzli-native-postgres' AND usename='webdock_auth_bootstrap' AND wait_event_type='Lock' AND query LIKE '%FOR UPDATE OF u,s%') AS native_management")).await.unwrap().unwrap().try_get::<bool>("","native_management").unwrap();
            if active {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "actual management HTTP must execute native canonical row locking"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        gate.commit().await.unwrap();
        let response = request.await.unwrap();
        assert_eq!(response.status(), 200);
        let rows: Value = response.json().await.unwrap();
        let rows = rows.as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r["id"] != "foreign-session"));
        for (path, body) in [
            ("revoke-session-by-id", json!({"id":"foreign-session"})),
            ("revoke-other-sessions", json!({})),
            ("sign-out", json!({})),
        ] {
            let response = client
                .post(format!("http://127.0.0.1:{port}/api/auth/{path}"))
                .header("Host", "localhost:3125")
                .header("Origin", "http://localhost:3125")
                .header("Cookie", auth_cookie.clone())
                .json(&body)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
        }
        let counts=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT count(*) FILTER(WHERE id='foreign-session') AS foreign_count,count(*) FILTER(WHERE \"userId\"='owned-session-user') AS own_count FROM webdock_auth.session")).await.unwrap().unwrap();
        assert_eq!(counts.try_get::<i64>("", "foreign_count").unwrap(), 1);
        assert_eq!(counts.try_get::<i64>("", "own_count").unwrap(), 0);
        let response = client
            .get(format!("http://127.0.0.1:{port}/api/auth/get-session"))
            .header("Host", "localhost:3125")
            .header("Cookie", auth_cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert!(response.json::<Value>().await.unwrap().is_null());
    }
    if native_browser {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let response = client
            .get(format!(
                "http://127.0.0.1:{port}/api/sso/login?returnTo=%2Fprojects"
            ))
            .header("Host", "localhost:3121")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 303);
        let location = response
            .headers()
            .get("location")
            .unwrap()
            .to_str()
            .unwrap();
        let location = url::Url::parse(location).unwrap();
        assert_eq!(
            location.origin().ascii_serialization(),
            "http://localhost:3125"
        );
        let params = location
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(params["client_id"], "102");
        assert_eq!(params["code_challenge_method"], "S256");
        assert!(!params["state"].is_empty());
        assert!(!params["nonce"].is_empty());
        assert!(
            response
                .headers()
                .get("set-cookie")
                .unwrap()
                .to_str()
                .unwrap()
                .contains("HttpOnly")
        );
        let row=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT (SELECT count(*) FROM public.owned_browser_http_audit WHERE app='spitzli-native-postgres') AS native_writes,(SELECT payload FROM webdock_auth.identity_browser_flow LIMIT 1) AS payload")).await.unwrap().unwrap();
        assert_eq!(row.try_get::<i64>("", "native_writes").unwrap(), 1);
        assert!(
            !row.try_get::<String>("", "payload")
                .unwrap()
                .contains(params["state"].as_ref())
        );
        if native_authority {
            let flow_cookie = response
                .headers()
                .get_all("set-cookie")
                .iter()
                .map(|v| v.to_str().unwrap())
                .find(|v| v.starts_with("webdock-console.flow="))
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .to_owned();
            let auth_cookie = CookiePolicy::new(false, secret)
                .unwrap()
                .header("owned-introspection-session", 3600)
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .to_owned();
            let authorize_query = if native_tokens {
                let mut requested = location.clone();
                let pairs = requested
                    .query_pairs()
                    .filter(|(k, _)| k != "scope")
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect::<Vec<_>>();
                requested.set_query(None);
                requested
                    .query_pairs_mut()
                    .extend_pairs(pairs)
                    .append_pair("scope", "openid profile email offline_access");
                requested.query().unwrap().to_owned()
            } else {
                location.query().unwrap().to_owned()
            };
            let authorize = client
                .get(format!(
                    "http://127.0.0.1:{port}/api/auth/oauth2/authorize?{}",
                    authorize_query
                ))
                .header("Host", "localhost:3125")
                .header("Cookie", auth_cookie)
                .send()
                .await
                .unwrap();
            assert!(
                authorize.status().is_redirection(),
                "owned authorization must redirect"
            );
            let callback = url::Url::parse(
                authorize
                    .headers()
                    .get("location")
                    .unwrap()
                    .to_str()
                    .unwrap(),
            )
            .unwrap();
            let response = client
                .get(format!(
                    "http://127.0.0.1:{port}/api/sso/callback?{}",
                    callback.query().unwrap()
                ))
                .header("Host", "localhost:3121")
                .header("Cookie", flow_cookie)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 303);
            let signed = response
                .headers()
                .get_all("set-cookie")
                .iter()
                .map(|v| v.to_str().unwrap())
                .find(|v| v.starts_with("webdock-console.session="))
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .to_owned();
            let gate = owner.begin().await.unwrap();
            gate.execute_unprepared(
                "SELECT token_hash FROM webdock_auth.identity_browser_session FOR UPDATE",
            )
            .await
            .unwrap();
            let http = client.clone();
            let c = signed.clone();
            let request = tokio::spawn(async move {
                http.get(format!(
                    "http://127.0.0.1:{port}/api/console/auth/get-session"
                ))
                .header("Host", "localhost:3121")
                .header("Cookie", c)
                .send()
                .await
                .unwrap()
            });
            let until = tokio::time::Instant::now() + Duration::from_secs(3);
            loop {
                let waiting=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name='spitzli-native-postgres' AND wait_event_type='Lock' AND query LIKE '%identity_browser_session%' AND query LIKE '%FOR SHARE%') AS waiting")).await.unwrap().unwrap().try_get::<bool>("","waiting").unwrap();
                if waiting {
                    break;
                }
                assert!(
                    tokio::time::Instant::now() < until,
                    "actual cookie resolution must use native browser proof"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            gate.commit().await.unwrap();
            let response = request.await.unwrap();
            assert_eq!(response.status(), 200);
            let profile: Value = response.json().await.unwrap();
            assert_eq!(profile["user"]["id"], "501");
            assert_eq!(profile["user"]["email"], "http@example.test");
            if native_tokens {
                let raw = owner
                    .query_one_raw(Statement::from_string(
                        DbBackend::Postgres,
                        "SELECT payload FROM webdock_auth.identity_browser_session LIMIT 1",
                    ))
                    .await
                    .unwrap()
                    .unwrap()
                    .try_get::<String>("", "payload")
                    .unwrap();
                let keys = spitzli_identity::SecretKeys::new(
                    std::collections::BTreeMap::from([(1, secret.into())]),
                    1,
                    Some(secret.into()),
                )
                .unwrap();
                let stored: Value = serde_json::from_str(&keys.decrypt(&raw).unwrap()).unwrap();
                let refresh = stored["refresh_token"].as_str().unwrap();
                let response = client
                    .post(format!("http://127.0.0.1:{port}/api/auth/oauth2/token"))
                    .header("Host", "localhost:3125")
                    .form(&[
                        ("grant_type", "refresh_token"),
                        ("client_id", "102"),
                        ("client_secret", "owned-core-browser-secret"),
                        ("refresh_token", refresh),
                    ])
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), 200);
                let result: Value = response.json().await.unwrap();
                assert!(result["access_token"].as_str().is_some());
                assert!(result["refresh_token"].as_str().is_some());
                assert!(result["id_token"].as_str().is_some());
                let row=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT count(*) FILTER(WHERE kind='oauthAccessToken') AS access,count(*) FILTER(WHERE kind='oauthRefreshToken') AS refresh FROM public.owned_oauth_token_audit WHERE app='spitzli-native-postgres'")).await.unwrap().unwrap();
                assert_eq!(row.try_get::<i64>("", "access").unwrap(), 2);
                assert_eq!(row.try_get::<i64>("", "refresh").unwrap(), 2);
            }
            owner
                .execute_unprepared("DELETE FROM webdock_auth.identity_browser_session")
                .await
                .unwrap();
            let response = client
                .get(format!(
                    "http://127.0.0.1:{port}/api/console/auth/get-session"
                ))
                .header("Host", "localhost:3121")
                .header("Cookie", signed)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            assert!(response.json::<Value>().await.unwrap().is_null());
        }
    }
    if native_oauth {
        let gate = owner.begin().await.unwrap();
        gate.execute_unprepared("LOCK TABLE webdock_auth.jwks IN ACCESS EXCLUSIVE MODE")
            .await
            .unwrap();
        let request_client = client.clone();
        let request = tokio::spawn(async move {
            request_client
                .get(format!("http://127.0.0.1:{port}/api/auth/jwks"))
                .header("Host", "localhost:3125")
                .timeout(Duration::from_secs(5))
                .send()
                .await
                .unwrap()
        });
        let until = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let waiting=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name='spitzli-native-postgres' AND usename='webdock_auth_bootstrap' AND wait_event_type='Lock' AND query LIKE '%FROM webdock_auth.jwks%') AS native_jwks")).await.unwrap().unwrap().try_get::<bool>("","native_jwks").unwrap();
            if waiting {
                break;
            }
            assert!(
                tokio::time::Instant::now() < until,
                "actual public JWKS must use native read"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        gate.commit().await.unwrap();
        let response = request.await.unwrap();
        assert_eq!(response.status(), 200);
        let keys: Value = response.json().await.unwrap();
        assert_eq!(keys["keys"].as_array().unwrap().len(), 1);
        assert_eq!(keys["keys"][0]["kid"], "301");
        assert!(keys["keys"][0].get("d").is_none());
    }
    if native_introspection {
        let gate = owner.begin().await.unwrap();
        gate.execute_unprepared(
            "LOCK TABLE webdock_auth.\"oauthAccessToken\" IN ACCESS EXCLUSIVE MODE",
        )
        .await
        .unwrap();
        let http = client.clone();
        let request = tokio::spawn(async move {
            http.post(format!(
                "http://127.0.0.1:{port}/api/auth/oauth2/introspect"
            ))
            .header("Host", "localhost:3125")
            .form(&[
                ("client_id", "102"),
                ("client_secret", "owned-core-browser-secret"),
                ("token", "owned-http-access"),
            ])
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .unwrap()
        });
        let until = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let waiting=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name='spitzli-native-postgres' AND usename='webdock_auth_bootstrap' AND wait_event_type='Lock' AND query LIKE '%oauthAccessToken%') AS blocked")).await.unwrap().unwrap().try_get::<bool>("","blocked").unwrap();
            if waiting {
                break;
            }
            assert!(
                tokio::time::Instant::now() < until,
                "actual introspection must use native driver"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        gate.commit().await.unwrap();
        let response = request.await.unwrap();
        assert_eq!(response.status(), 200);
        let claims: Value = response.json().await.unwrap();
        assert_eq!(claims["active"], true);
        assert_eq!(claims["sub"], "501");
        assert_eq!(claims["webdock_role"], "reader");
        let response = client
            .get(format!("http://127.0.0.1:{port}/api/auth/oauth2/userinfo"))
            .header("Host", "localhost:3125")
            .bearer_auth("owned-http-access")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let profile: Value = response.json().await.unwrap();
        assert_eq!(profile["email"], "http@example.test");
        if native_security {
            let gate = owner.begin().await.unwrap();
            gate.execute_unprepared(
                "SELECT id FROM webdock_auth.\"user\" WHERE id='501' FOR UPDATE",
            )
            .await
            .unwrap();
            let http = client.clone();
            let request = tokio::spawn(async move {
                http.post(format!("http://127.0.0.1:{port}/api/auth/oauth2/revoke"))
                    .header("Host", "localhost:3125")
                    .form(&[
                        ("client_id", "102"),
                        ("client_secret", "owned-core-browser-secret"),
                        ("token", "owned-http-access"),
                    ])
                    .timeout(Duration::from_secs(5))
                    .send()
                    .await
                    .unwrap()
            });
            let until = tokio::time::Instant::now() + Duration::from_secs(3);
            loop {
                let waiting=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name='spitzli-native-postgres' AND wait_event_type='Lock' AND query LIKE '%webdock_auth.\"user\"%' AND query LIKE '%FOR UPDATE%') AS waiting")).await.unwrap().unwrap().try_get::<bool>("","waiting").unwrap();
                if waiting {
                    break;
                }
                assert!(
                    tokio::time::Instant::now() < until,
                    "native revocation owner fence not observed"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            gate.commit().await.unwrap();
            assert_eq!(request.await.unwrap().status(), 200);
        } else {
            owner
                .execute_unprepared(
                    "UPDATE webdock_auth.\"oauthAccessToken\" SET revoked=now() WHERE id='504'",
                )
                .await
                .unwrap();
        }
        let response = client
            .post(format!(
                "http://127.0.0.1:{port}/api/auth/oauth2/introspect"
            ))
            .header("Host", "localhost:3125")
            .form(&[
                ("client_id", "102"),
                ("client_secret", "owned-core-browser-secret"),
                ("token", "owned-http-access"),
            ])
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.json::<Value>().await.unwrap()["active"], false);
    }
    if native_grants {
        let n=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT count(*) AS n FROM public.owned_oauth_grant_audit WHERE app='spitzli-native-postgres'")).await.unwrap().unwrap().try_get::<i64>("","n").unwrap();
        assert_eq!(n, 1, "actual HTTP code write must use native driver");
    }
    if native_mcp {
        let gate = owner.begin().await.unwrap();
        gate.execute_unprepared(
            "SELECT id FROM webdock_auth.\"oauthAccessToken\" WHERE id='903' FOR UPDATE",
        )
        .await
        .unwrap();
        let http = client.clone();
        let request = tokio::spawn(async move {
            http.get(format!("http://127.0.0.1:{port}/api/hosting/not-valid"))
                .header("Host", "localhost:3120")
                .bearer_auth("owned-http-mcp")
                .timeout(Duration::from_secs(5))
                .send()
                .await
                .unwrap()
        });
        let until = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let waiting=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name='spitzli-native-postgres' AND wait_event_type='Lock' AND query LIKE '%oauthAccessToken%' AND query LIKE '%FOR SHARE%') AS waiting")).await.unwrap().unwrap().try_get::<bool>("","waiting").unwrap();
            if waiting {
                break;
            }
            assert!(
                tokio::time::Instant::now() < until,
                "actual resource boundary must retain native access fence"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        gate.commit().await.unwrap();
        assert_eq!(
            request.await.unwrap().status(),
            404,
            "valid authentication reaches routing contract"
        );
        owner
            .execute_unprepared(
                "UPDATE webdock_auth.\"oauthAccessToken\" SET revoked=now() WHERE id='903'",
            )
            .await
            .unwrap();
        let response = client
            .get(format!("http://127.0.0.1:{port}/api/hosting/not-valid"))
            .header("Host", "localhost:3120")
            .bearer_auth("owned-http-mcp")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 401);
        assert!(response.headers().contains_key("www-authenticate"));
    }
    assert_eq!(
        before,
        journal(&owner).await,
        "startup must not mutate migration history"
    );
    assert!(
        Command::new("kill")
            .args(["-TERM", &process.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = process.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "native Core ignored SIGTERM"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(before, journal(&owner).await);
    let after_accounts: Value = owner
        .query_one_raw(account_counts())
        .await
        .unwrap()
        .unwrap()
        .try_get("", "counts")
        .unwrap();
    assert_eq!(
        before_accounts, after_accounts,
        "startup must not seed customer or identity records"
    );
    assert!(!std::fs::read_to_string(&log).unwrap().contains(&password));
    command.env("WEBDOCK_DATABASE_URL", fixture.url.as_str());
    let mut rejected = Process(command.spawn().unwrap());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = rejected.0.try_wait().unwrap() {
            assert!(!status.success());
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "privileged runtime was not refused"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        std::fs::read_to_string(log)
            .unwrap()
            .contains("Restricted database role required")
    );
}

#[tokio::test]
async fn sigterm_cancels_stalled_database_initialization() {
    let stalled = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = stalled.local_addr().unwrap().port();
    let mut process=Process(Command::new(env!("CARGO_BIN_EXE_spitzli-control-plane"))
        .env_clear().env("PATH",std::env::var("PATH").unwrap())
        .env("NODE_ENV","development")
        .env("WEBDOCK_DATABASE_URL",format!("postgres://webdock_auth_bootstrap:fixture@127.0.0.1:{port}/stalled?sslmode=disable"))
        .stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
    // Accept but never answer PostgreSQL startup: no schema/data or actual provider is involved.
    let (_socket, _) = tokio::time::timeout(Duration::from_secs(5), stalled.accept())
        .await
        .unwrap()
        .unwrap();
    assert!(
        Command::new("kill")
            .args(["-TERM", &process.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(status) = process.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "SIGTERM was swallowed during database startup"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
