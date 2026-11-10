//! Actual Managed provider; only the pinned disposable all-user qualification branch.
use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};
use serde::Deserialize;
use spitzli_identity::{
    CookiePolicy, IdentityService, NeonAuthClient, PasswordAuthentication, RlsSigner, SecretKeys,
};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    branch_id: String,
    owner_url: String,
    bootstrap_url: String,
    runtime_url: String,
    rls_secret: String,
    provider_url: String,
    auth_secret: String,
    reset_email: String,
    reset_password: String,
}
fn cookie(value: &str) -> &str {
    value.split(';').next().unwrap()
}

#[tokio::test]
#[ignore = "owned child-only synthetic factor custody for immutable image qualification"]
async fn prepare_owned_member_image_fixture() {
    let path = std::env::var("WEBDOCK_NEON_ALL_USERS_FIXTURE").unwrap();
    let f: Fixture = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(f.branch_id, "br-ancient-leaf-b9izc4dw");
    assert!(
        url::Url::parse(&f.owner_url)
            .unwrap()
            .host_str()
            .unwrap()
            .starts_with("ep-lucky-base-")
    );
    assert_eq!(f.reset_email, "owned-reset-20261031@example.invalid");
    let owner = Database::connect(&f.owner_url).await.unwrap();
    let row = owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT u.id,f.secret FROM webdock_auth.\"user\" u JOIN webdock_auth.\"twoFactor\" f ON f.\"userId\"=u.id WHERE u.email=$1 AND u.role='user' AND u.\"twoFactorEnabled\"", [f.reset_email.clone().into()])).await.unwrap().unwrap();
    let keys = SecretKeys::new(
        BTreeMap::from([(1, f.auth_secret.clone())]),
        1,
        Some(f.auth_secret.clone()),
    )
    .unwrap();
    let secret = keys
        .decrypt(&row.try_get::<String>("", "secret").unwrap())
        .unwrap();
    let totp = totp_rs::Builder::new()
        .with_secret(secret.as_bytes().to_vec())
        .with_algorithm(totp_rs::Algorithm::SHA1)
        .with_digits(6)
        .with_skew(1)
        .with_step_duration(30)
        .with_issuer(Some("Owned qualification"))
        .with_account_name(f.reset_email.as_str())
        .build()
        .unwrap();
    let uri = totp.to_url().unwrap();
    assert_eq!(
        totp_rs::Totp::from_url(&uri)
            .unwrap()
            .generate_current()
            .to_string(),
        totp.generate_current().to_string()
    );
    let value = serde_json::json!({"userID":row.try_get::<String>("","id").unwrap(),"email":f.reset_email,"password":f.reset_password,"totpURI":uri,"role":"user"});
    let output = std::path::Path::new(&path).with_file_name("owned-image-user.private.json");
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(output)
        .unwrap();
    file.write_all(value.to_string().as_bytes()).unwrap();
}

#[tokio::test]
#[ignore = "owned child-only scoped automation exception and explicit revocation"]
async fn scoped_automation_remains_independent_of_interactive_enforcement() {
    let f: Fixture = serde_json::from_slice(
        &std::fs::read(std::env::var("WEBDOCK_NEON_ALL_USERS_FIXTURE").unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(f.branch_id, "br-ancient-leaf-b9izc4dw");
    let url = url::Url::parse(&f.owner_url).unwrap();
    assert!(url.host_str().unwrap().starts_with("ep-lucky-base-"));
    assert_eq!(
        url::Url::parse(&f.bootstrap_url).unwrap().host_str(),
        url.host_str()
    );
    let owner = Database::connect(&f.owner_url).await.unwrap();
    let auth = Database::connect(&f.bootstrap_url).await.unwrap();
    let ids = Arc::new(
        spitzli_persistence::IdAllocator::connect(
            &f.bootstrap_url,
            spitzli_ids::Identity::platform(0, 2).unwrap(),
        )
        .await
        .unwrap(),
    );
    let service = IdentityService::new(auth, CookiePolicy::new(true, &f.auth_secret).unwrap())
        .with_allocator(ids.clone());
    let email = format!(
        "owned-automation-{}@example.invalid",
        ids.allocate().await.unwrap()
    );
    let user = service
        .create_local_account(
            &email,
            "Owned automation",
            "Owned-placeholder-password-9384",
            false,
        )
        .await
        .unwrap();
    owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.\"user\" SET \"emailVerified\"=true,\"mustChangePassword\"=false WHERE id=$1",[user.clone().into()])).await.unwrap();
    owner.execute_raw(Statement::from_string(DbBackend::Postgres,"UPDATE webdock_auth.neon_auth_policy SET require_managed=true,activated_by='99866772345196544',activated_at=statement_timestamp() WHERE singleton")).await.unwrap();
    let session = service
        .issue_api_session(&user, "Owned scoped automation", 1)
        .await
        .unwrap();
    let token = ids.allocate().await.unwrap().to_string();
    owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.api_token(id,customer_id,session_id,user_id,name,token_hash,can_write,expires_at) SELECT $1,(SELECT id FROM webdock_admin.customers ORDER BY id LIMIT 1),$2,$3,'Owned automation',repeat('a',64),false,statement_timestamp()+interval '1 day'",[token.clone().into(),session.clone().into(),user.clone().into()])).await.unwrap();
    let live = |user: String, session: String| {
        Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT webdock_rls.native_provider_session_live($1,$2) AS live",
            [user.into(), session.into()],
        )
    };
    assert!(
        owner
            .query_one_raw(live(user.clone(), session.clone()))
            .await
            .unwrap()
            .unwrap()
            .try_get::<bool>("", "live")
            .unwrap()
    );
    assert!(
        service
            .authenticate_password(&email, "Owned-placeholder-password-9384")
            .await
            .is_err(),
        "automation allowed an interactive native login"
    );
    owner
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.api_token WHERE id=$1",
            [token.into()],
        ))
        .await
        .unwrap();
    assert!(
        !owner
            .query_one_raw(live(user, session))
            .await
            .unwrap()
            .unwrap()
            .try_get::<bool>("", "live")
            .unwrap(),
        "revoked automation retained authority"
    );
}

#[tokio::test]
#[ignore = "owned all-user Neon child only; creates synthetic member, never production"]
async fn retained_customer_mfa_is_a_session_and_rls_boundary() {
    let path = std::env::var("WEBDOCK_NEON_ALL_USERS_FIXTURE").unwrap();
    let f: Fixture = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(f.branch_id, "br-ancient-leaf-b9izc4dw");
    let url = url::Url::parse(&f.owner_url).unwrap();
    assert!(url.host_str().unwrap().starts_with("ep-lucky-base-"));
    assert_eq!(url.query(), Some("sslmode=verify-full"));
    for value in [&f.bootstrap_url, &f.runtime_url] {
        let role_url = url::Url::parse(value).unwrap();
        assert_eq!(role_url.host_str(), url.host_str());
        assert_eq!(role_url.path(), url.path());
        assert_eq!(role_url.query(), Some("sslmode=verify-full"));
    }
    assert!(
        f.provider_url
            .starts_with("https://ep-lucky-base-b9eaymxz.neonauth.")
    );
    assert_eq!(f.reset_email, "owned-reset-20261031@example.invalid");
    let owner = Database::connect(&f.owner_url).await.unwrap();
    let auth = Database::connect(&f.bootstrap_url).await.unwrap();
    let runtime = Database::connect(&f.runtime_url).await.unwrap();
    let allocator = Arc::new(
        spitzli_persistence::IdAllocator::connect(
            &f.bootstrap_url,
            spitzli_ids::Identity::platform(0, 2).unwrap(),
        )
        .await
        .unwrap(),
    );
    let service = IdentityService::new(
        auth.clone(),
        CookiePolicy::new(true, &f.auth_secret).unwrap(),
    )
    .with_allocator(allocator.clone())
    .with_secret_keys(
        SecretKeys::new(
            BTreeMap::from([(1, f.auth_secret.clone())]),
            1,
            Some(f.auth_secret.clone()),
        )
        .unwrap(),
    )
    .with_neon_auth(NeonAuthClient::new(&f.provider_url, "https://auth.webdock.dev").unwrap());
    let policy_exists: bool = owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT to_regclass('webdock_auth.neon_auth_policy') IS NOT NULL AS present",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "present")
        .unwrap();
    if policy_exists {
        owner.execute_raw(Statement::from_string(DbBackend::Postgres,
            "UPDATE webdock_auth.neon_auth_policy SET require_managed=false,activated_by=NULL,activated_at=NULL")).await.unwrap();
    }
    for email in [
        &f.reset_email,
        &"owned-collision-20261031@example.invalid".to_owned(),
    ] {
        for statement in [
            "DELETE FROM webdock_auth.session WHERE \"userId\" IN (SELECT id FROM webdock_auth.\"user\" WHERE email=$1)",
            "DELETE FROM webdock_auth.neon_identity WHERE user_id IN (SELECT id FROM webdock_auth.\"user\" WHERE email=$1)",
            "DELETE FROM webdock_auth.\"user\" WHERE email=$1",
        ] {
            owner
                .execute_raw(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    statement,
                    [email.clone().into()],
                ))
                .await
                .unwrap();
        }
    }
    let id = service
        .create_local_account(
            &f.reset_email,
            "Owned all-user member",
            &f.reset_password,
            false,
        )
        .await
        .unwrap();
    owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,
        "UPDATE webdock_auth.\"user\" SET \"emailVerified\"=true,\"mustChangePassword\"=false WHERE id=$1", [id.clone().into()])).await.unwrap();
    let PasswordAuthentication::Authenticated(native) = service
        .authenticate_password(&f.reset_email, &f.reset_password)
        .await
        .unwrap()
    else {
        panic!("approved native fixture required")
    };
    service
        .link_neon_identity(cookie(&native.set_cookie), &f.reset_password)
        .await
        .unwrap();
    let enrollment = service
        .begin_totp_enrollment(cookie(&native.set_cookie), &f.reset_password)
        .await
        .unwrap();
    let totp = totp_rs::Totp::from_url(&enrollment.totp_uri).unwrap();
    let full = service
        .confirm_totp_enrollment(
            cookie(&native.set_cookie),
            &totp.generate_current().to_string(),
        )
        .await
        .unwrap();
    assert_eq!(full.principal.role(), "user");
    assert!(full.principal.can_access());
    let session = full.principal.session_id.clone();
    owner
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_auth.neon_session SET mfa_verified=false WHERE session_id=$1",
            [session.clone().into()],
        ))
        .await
        .unwrap();
    let denied = service
        .authenticate_cookie(cookie(&full.set_cookie))
        .await
        .unwrap()
        .is_none();
    let row = owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT webdock_rls.native_provider_session_live($1,$2) AS live",
            [id.clone().into(), session.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap();
    let live: bool = row.try_get("", "live").unwrap();
    // Restore only the owned fixture marker even when an assertion below fails.
    owner
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_auth.neon_session SET mfa_verified=true WHERE session_id=$1",
            [session.into()],
        ))
        .await
        .unwrap();
    assert!(
        denied,
        "MFA-enabled customer accepted a provider binding without verified MFA"
    );
    assert!(
        !live,
        "signed RLS admitted a customer binding without verified MFA"
    );
    // Provider first factor cannot bypass a missing native approval.
    owner
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_auth.\"user\" SET role='unapproved' WHERE id=$1",
            [id.clone().into()],
        ))
        .await
        .unwrap();
    assert!(
        service
            .authenticate_password(&f.reset_email, &f.reset_password)
            .await
            .is_err(),
        "unapproved customer received a session/challenge"
    );
    owner
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_auth.\"user\" SET role='user' WHERE id=$1",
            [id.clone().into()],
        ))
        .await
        .unwrap();
    assert!(policy_exists, "all-user migration is required");
    let native_only = IdentityService::new(
        auth.clone(),
        CookiePolicy::new(true, &f.auth_secret).unwrap(),
    )
    .with_allocator(allocator.clone())
    .with_secret_keys(
        SecretKeys::new(
            BTreeMap::from([(1, f.auth_secret.clone())]),
            1,
            Some(f.auth_secret.clone()),
        )
        .unwrap(),
    );
    let email = format!(
        "all-users-native-{}@example.invalid",
        allocator.allocate().await.unwrap()
    );
    let password = "Owned-invitation-password-93847";
    let next = native_only
        .create_local_account(&email, "Owned new invitation", password, false)
        .await
        .unwrap();
    owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,
        "UPDATE webdock_auth.\"user\" SET \"emailVerified\"=true,\"mustChangePassword\"=false WHERE id=$1", [next.clone().into()])).await.unwrap();
    let PasswordAuthentication::Authenticated(old) = native_only
        .authenticate_password(&email, password)
        .await
        .unwrap()
    else {
        panic!("default-off compatibility failed")
    };
    owner.execute_raw(Statement::from_string(DbBackend::Postgres,
        "UPDATE webdock_auth.neon_auth_policy SET require_managed=true,activated_by='99866772345196544',activated_at=statement_timestamp() WHERE singleton")).await.unwrap();
    assert!(
        native_only
            .authenticate_cookie(cookie(&old.set_cookie))
            .await
            .unwrap()
            .is_none(),
        "global policy admitted an old unmapped cookie"
    );
    assert!(
        native_only
            .authenticate_password(&email, password)
            .await
            .is_err(),
        "provider outage/configuration produced a native session"
    );
    let PasswordAuthentication::Authenticated(bound) = service
        .authenticate_password(&email, password)
        .await
        .unwrap()
    else {
        panic!("new approved invitation did not enroll through Managed")
    };
    assert_eq!(bound.principal.user_id, next);
    assert!(
        service
            .authenticate_cookie(cookie(&bound.set_cookie))
            .await
            .unwrap()
            .unwrap()
            .can_access()
    );
    let own_org = allocator.allocate().await.unwrap().to_string();
    let other_org = allocator.allocate().await.unwrap().to_string();
    for org in [&own_org, &other_org] {
        owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "INSERT INTO webdock_auth.organization(id,name,slug,\"createdAt\") VALUES($1,'Owned scope fixture',$2,statement_timestamp())",
            [org.clone().into(),format!("owned-scope-{org}").into()])).await.unwrap();
    }
    owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,
        "INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES($1,$2,$3,'admin',statement_timestamp())",
        [allocator.allocate().await.unwrap().to_string().into(),own_org.clone().into(),bound.principal.user_id.clone().into()])).await.unwrap();
    let signer = RlsSigner::new(&f.rls_secret, "webdock_auth_runtime").unwrap();
    let scoped = signer.begin(&runtime, &bound.principal).await.unwrap();
    let scopes = scoped.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT webdock_rls.organization_allowed($1,false) AS owned,webdock_rls.organization_allowed($2,false) AS foreign_scope",
        [own_org.into(),other_org.into()])).await.unwrap().unwrap();
    assert!(scopes.try_get::<bool>("", "owned").unwrap());
    assert!(!scopes.try_get::<bool>("", "foreign_scope").unwrap());
    scoped.rollback().await.unwrap();
    let collision = "owned-collision-20261031@example.invalid";
    let collision_id = native_only
        .create_local_account(collision, "Owned collision fixture", password, false)
        .await
        .unwrap();
    owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,
        "UPDATE webdock_auth.\"user\" SET \"emailVerified\"=true,\"mustChangePassword\"=false WHERE id=$1", [collision_id.clone().into()])).await.unwrap();
    assert!(
        service
            .authenticate_password(collision, password)
            .await
            .is_err(),
        "existing provider account was adopted"
    );
    let absent = owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT NOT EXISTS(SELECT 1 FROM webdock_auth.neon_identity WHERE user_id=$1) AND NOT EXISTS(SELECT 1 FROM webdock_auth.session WHERE \"userId\"=$1) AS absent", [collision_id.into()])).await.unwrap().unwrap().try_get::<bool>("", "absent").unwrap();
    assert!(absent, "collision created a native or mapped session");
    assert!(
        auth.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.neon_identity WHERE user_id=$1",
            [next.clone().into()]
        ))
        .await
        .is_err(),
        "runtime deleted a binding"
    );
    assert!(
        auth.execute_raw(Statement::from_string(
            DbBackend::Postgres,
            "UPDATE webdock_auth.neon_auth_policy SET require_managed=false"
        ))
        .await
        .is_err(),
        "runtime weakened global policy"
    );
    assert!(owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,
        "UPDATE webdock_auth.neon_identity SET provider_subject='00000000-0000-0000-0000-000000000001' WHERE user_id=$1", [next.into()])).await.is_err(), "canonical provider subject changed");
    owner.execute_raw(Statement::from_string(DbBackend::Postgres,
        "UPDATE webdock_auth.neon_auth_policy SET require_managed=false,activated_by=NULL,activated_at=NULL")).await.unwrap();
}
