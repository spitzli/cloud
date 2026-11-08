//! Owned local PostgreSQL; no supplied DSN/provider/production configuration.
#[path = "support/native_pg.rs"]
mod fixture;
use fixture::{AUTH_SECRET, RLS_SECRET};
use sea_orm::{ConnectionTrait, Database};
use webdock_identity::{CookiePolicy, IdentityService, RlsSigner};
use webdock_persistence::postgres::{Limits, Pool};
async fn sql(db: &impl ConnectionTrait, text: &str) {
    db.execute_unprepared(text).await.unwrap();
}
fn cookie(token: &str) -> String {
    CookiePolicy::new(false, AUTH_SECRET)
        .unwrap()
        .header(token, 3600)
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .into()
}
async fn pool(url: &str, role: &str) -> Pool {
    Pool::connect(
        url,
        role,
        false,
        Limits {
            max_connections: 1,
            min_connections: 0,
            ..Default::default()
        },
    )
    .await
    .unwrap()
}
async fn service(f: &fixture::Fixture, native: bool) -> IdentityService {
    let service = IdentityService::new(
        Database::connect(&f.bootstrap_url).await.unwrap(),
        CookiePolicy::new(false, AUTH_SECRET).unwrap(),
    );
    if native {
        service.with_native_session_reads(pool(&f.bootstrap_url, "webdock_auth_bootstrap").await)
    } else {
        service
    }
}
async fn seed(f: &fixture::Fixture) {
    sql(&f.owner, r#"INSERT INTO webdock_auth."user"(id,name,email,"emailVerified",role,"twoFactorEnabled","mustChangePassword") VALUES
    ('a','A','a@example.test',true,'user',false,false),('b','B','b@example.test',true,'user',false,false),('backup','Backup','backup@example.test',true,'user',false,false),('op','Operator','op@example.test',true,'operator',true,false);
    INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES
    ('sa','owned-a-token','a',now()+interval '1 hour',now()),('sb','owned-b-token','b',now()+interval '1 hour',now()),('sop','owned-op-token','op',now()+interval '1 hour',now())"#).await;
}
async fn parity(
    legacy: &IdentityService,
    native: &IdentityService,
    token: &str,
) -> Option<webdock_identity::Principal> {
    let a = legacy.authenticate_cookie(&cookie(token)).await.unwrap();
    let b = native.authenticate_cookie(&cookie(token)).await.unwrap();
    assert_eq!(
        serde_json::to_value(&a).unwrap(),
        serde_json::to_value(&b).unwrap()
    );
    b
}
#[tokio::test]
#[ignore = "owned local PostgreSQL18; restricted native session read parity"]
async fn native_session_reads_preserve_cookie_setup_mfa_and_revocation() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let legacy = service(&f, false).await;
    let native = service(&f, true).await;
    assert!(
        parity(&legacy, &native, "owned-a-token")
            .await
            .unwrap()
            .can_access()
    );
    assert!(
        parity(&legacy, &native, "owned-op-token")
            .await
            .unwrap()
            .is_operator()
    );
    sql(
        &f.owner,
        r#"UPDATE webdock_auth."user" SET "twoFactorEnabled"=false WHERE id='op'"#,
    )
    .await;
    assert!(
        !parity(&legacy, &native, "owned-op-token")
            .await
            .unwrap()
            .can_access()
    );
    sql(
        &f.owner,
        r#"UPDATE webdock_auth."user" SET "mustChangePassword"=true WHERE id='a'"#,
    )
    .await;
    assert!(
        parity(&legacy, &native, "owned-a-token")
            .await
            .unwrap()
            .requires_password_change()
    );
    sql(&f.owner,r#"UPDATE webdock_auth."user" SET "mustChangePassword"=false,"emailVerified"=false WHERE id='a'"#).await;
    assert!(
        !parity(&legacy, &native, "owned-a-token")
            .await
            .unwrap()
            .can_access()
    );
    sql(
        &f.owner,
        r#"UPDATE webdock_auth."user" SET "emailVerified"=true,banned=true WHERE id='a'"#,
    )
    .await;
    assert!(parity(&legacy, &native, "owned-a-token").await.is_none());
    sql(&f.owner,r#"UPDATE webdock_auth."user" SET banned=false WHERE id='a';UPDATE webdock_auth.session SET "expiresAt"=now()-interval '1 second' WHERE id='sa'"#).await;
    assert!(parity(&legacy, &native, "owned-a-token").await.is_none());
    sql(
        &f.owner,
        r#"UPDATE webdock_auth.session SET "expiresAt"=now()+interval '1 hour' WHERE id='sa'"#,
    )
    .await;
    assert!(
        native
            .authenticate_cookie("webdock-auth.session_token=forged")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        native
            .authenticate_cookie(&cookie("missing-token"))
            .await
            .unwrap()
            .is_none()
    );
    sql(&f.owner,"ALTER FUNCTION webdock_rls.native_provider_session_live(text,text) RENAME TO owned_missing_policy").await;
    assert!(
        native
            .authenticate_cookie(&cookie("owned-a-token"))
            .await
            .is_err(),
        "missing authoritative policy must fail closed without legacy fallback"
    );
    sql(&f.owner,"ALTER FUNCTION webdock_rls.owned_missing_policy(text,text) RENAME TO native_provider_session_live").await;
    sql(&f.owner, "ALTER ROLE webdock_auth_bootstrap SUPERUSER").await;
    assert!(
        native
            .authenticate_cookie(&cookie("owned-a-token"))
            .await
            .is_err()
    );
}
#[tokio::test]
#[ignore = "owned local PostgreSQL18; native signed RLS tenant scope"]
async fn native_signed_scope_preserves_tenant_isolation_and_live_revocation() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    sql(&f.owner,r#"WITH hint AS MATERIALIZED(SELECT set_config('webdock.organization_allocation','{"customer_id":"100","organization_id":"101"}',true)) INSERT INTO webdock_admin.customers(id,name) SELECT '100','A' FROM hint;
    WITH hint AS MATERIALIZED(SELECT set_config('webdock.organization_allocation','{"customer_id":"200","organization_id":"201"}',true)) INSERT INTO webdock_admin.customers(id,name) SELECT '200','B' FROM hint;
    INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES('110','A','100'),('210','B','200');
    INSERT INTO webdock_auth.member(id,"organizationId","userId",role,"createdAt") VALUES('ma','101','a','owner',now()),('mc','101','backup','owner',now()),('mb','201','b','owner',now())"#).await;
    let legacy = service(&f, false).await;
    let identity = service(&f, true).await;
    sql(&f.owner,"INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id) VALUES('sop','op','100','101')").await;
    let preview = parity(&legacy, &identity, "owned-op-token").await.unwrap();
    assert!(preview.in_read_only_preview());
    assert!(!preview.is_operator());
    assert!(preview.customer_scope().is_some());
    sql(
        &f.owner,
        "UPDATE webdock_auth.studio_tenant_preview SET mode='owner' WHERE session_id='sop'",
    )
    .await;
    assert!(
        parity(&legacy, &identity, "owned-op-token")
            .await
            .unwrap()
            .is_acting_customer()
    );
    sql(&f.owner,"UPDATE webdock_auth.studio_tenant_preview SET expires_at=now()-interval '1 second' WHERE session_id='sop'").await;
    let expired = parity(&legacy, &identity, "owned-op-token").await.unwrap();
    assert!(expired.in_tenant_preview());
    assert!(!expired.is_operator());
    assert!(!expired.is_acting_customer());
    let principal = identity
        .authenticate_cookie(&cookie("owned-a-token"))
        .await
        .unwrap()
        .unwrap();
    let runtime = pool(&f.runtime_url, "webdock_auth_runtime").await;
    let signer = RlsSigner::new(RLS_SECRET, "webdock_auth_runtime").unwrap();
    let mut connection = runtime.acquire().await.unwrap();
    let tx = connection.transaction().await.unwrap();
    let tx = signer.apply_native(tx, &principal).await.unwrap();
    let rows = tx
        .query(
            "SELECT id,customer_id FROM webdock_admin.projects ORDER BY id",
            &[],
        )
        .await
        .unwrap();
    assert!(rows.iter().any(|r| r.get::<_, String>(0) == "110"));
    assert!(
        rows.iter().all(|r| r.get::<_, String>(1) == "100"),
        "only the signed member's tenant, including its automatic default project, may be visible"
    );
    tx.commit().await.unwrap();
    drop(connection);
    let mut connection = runtime.acquire().await.unwrap();
    let tx = connection.transaction().await.unwrap();
    let clean = tx
        .query_one(
            "SELECT coalesce(current_setting('webdock.rls_context',true),'')",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(clean.get::<_, String>(0), "");
    tx.rollback().await.unwrap();
    drop(connection);
    let mut connection = runtime.acquire().await.unwrap();
    let tx = connection.transaction().await.unwrap();
    assert!(
        RlsSigner::new(RLS_SECRET, "webdock_registry_runtime")
            .unwrap()
            .apply_native(tx, &principal)
            .await
            .is_err(),
        "signer audience must match actual checked role"
    );
    drop(connection);
    let mut connection = runtime.acquire().await.unwrap();
    let tx = connection.transaction().await.unwrap();
    let tx = RlsSigner::new("owned-wrong-signing-secret-32bytes", "webdock_auth_runtime")
        .unwrap()
        .apply_native(tx, &principal)
        .await
        .unwrap();
    let denied = tx.query("SELECT id FROM webdock_admin.projects", &[]).await;
    assert!(
        denied.is_err() || denied.unwrap().is_empty(),
        "invalid signed authority cannot grant tenant access"
    );
    tx.rollback().await.unwrap();
    drop(connection);
    sql(&f.owner, "DELETE FROM webdock_auth.member WHERE id='ma'").await;
    let mut connection = runtime.acquire().await.unwrap();
    let tx = connection.transaction().await.unwrap();
    let tx = signer.apply_native(tx, &principal).await.unwrap();
    assert!(
        tx.query("SELECT id FROM webdock_admin.projects", &[])
            .await
            .unwrap()
            .is_empty(),
        "stale Principal cannot retain revoked membership"
    );
    tx.rollback().await.unwrap();
    drop(connection);
    sql(&f.owner, "DELETE FROM webdock_auth.session WHERE id='sa'").await;
    let mut connection = runtime.acquire().await.unwrap();
    let tx = connection.transaction().await.unwrap();
    let tx = signer.apply_native(tx, &principal).await.unwrap();
    assert!(
        tx.query("SELECT id FROM webdock_admin.projects", &[])
            .await
            .unwrap()
            .is_empty(),
        "SQL rechecks canonical session revocation"
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
#[ignore = "owned local PostgreSQL18; synthetic provider policy tables, no provider network"]
async fn native_reader_rechecks_provider_binding_session_credentials_and_mfa() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    sql(&f.owner,r#"CREATE SCHEMA neon_auth;
    CREATE TABLE neon_auth."user"(id uuid PRIMARY KEY,banned boolean);
    CREATE TABLE neon_auth.session(id uuid PRIMARY KEY,"userId" uuid,"expiresAt" timestamptz,"impersonatedBy" uuid);
    CREATE TABLE neon_auth.account("userId" uuid,"providerId" text,password text);
    GRANT USAGE ON SCHEMA neon_auth TO webdock_platform_owner;
    GRANT SELECT ON ALL TABLES IN SCHEMA neon_auth TO webdock_platform_owner;
    INSERT INTO neon_auth."user" VALUES('00000000-0000-4000-8000-000000000001',false);
    INSERT INTO neon_auth.session VALUES('00000000-0000-4000-8000-000000000002','00000000-0000-4000-8000-000000000001',now()+interval '1 hour',null);
    INSERT INTO neon_auth.account VALUES('00000000-0000-4000-8000-000000000001','credential','owned-hash');
    INSERT INTO webdock_auth.neon_identity(user_id,provider_origin,provider_subject,linked_by) VALUES('a','https://provider.example.test/auth','00000000-0000-4000-8000-000000000001','op');
    INSERT INTO webdock_auth.neon_session(session_id,user_id,provider_session_id,provider_subject,identity_revision,credential_digest,encrypted_cookie,mfa_verified) VALUES('sa','a','00000000-0000-4000-8000-000000000002','00000000-0000-4000-8000-000000000001',1,encode(sha256(convert_to('owned-hash','UTF8')),'hex'),'owned-custody',false)"#).await;
    let legacy = service(&f, false).await;
    let native = service(&f, true).await;
    assert!(parity(&legacy, &native, "owned-a-token").await.is_some());
    for (revoke, restore) in [
        (
            "UPDATE neon_auth.session SET \"expiresAt\"=now()-interval '1 second'",
            "UPDATE neon_auth.session SET \"expiresAt\"=now()+interval '1 hour'",
        ),
        (
            "UPDATE neon_auth.\"user\" SET banned=true",
            "UPDATE neon_auth.\"user\" SET banned=false",
        ),
        (
            "UPDATE neon_auth.session SET \"impersonatedBy\"='00000000-0000-4000-8000-000000000001'",
            "UPDATE neon_auth.session SET \"impersonatedBy\"=null",
        ),
        (
            "UPDATE neon_auth.account SET password='changed-hash'",
            "UPDATE neon_auth.account SET password='owned-hash'",
        ),
        (
            "UPDATE webdock_auth.neon_identity SET revision=2 WHERE user_id='a'",
            "UPDATE webdock_auth.neon_session SET identity_revision=2 WHERE user_id='a'",
        ),
        (
            "UPDATE webdock_auth.neon_identity SET enabled=false,revision=3 WHERE user_id='a'",
            "UPDATE webdock_auth.neon_identity SET enabled=true,revision=4 WHERE user_id='a';UPDATE webdock_auth.neon_session SET identity_revision=4 WHERE user_id='a'",
        ),
        (
            "UPDATE webdock_auth.\"user\" SET \"twoFactorEnabled\"=true WHERE id='a'",
            "UPDATE webdock_auth.neon_session SET mfa_verified=true WHERE user_id='a'",
        ),
    ] {
        sql(&f.owner, revoke).await;
        assert!(
            parity(&legacy, &native, "owned-a-token").await.is_none(),
            "revocation must remain live: {revoke}"
        );
        sql(&f.owner, restore).await;
        assert!(parity(&legacy, &native, "owned-a-token").await.is_some());
    }
    sql(&f.owner,"UPDATE webdock_auth.neon_auth_policy SET require_managed=true,activated_by='op',activated_at=now()").await;
    assert!(
        parity(&legacy, &native, "owned-b-token").await.is_none(),
        "unbound user cannot bypass provider enforcement"
    );
}

#[tokio::test]
#[ignore = "owned local PostgreSQL18; non-gating session latency experiment"]
async fn compare_session_read_latency_without_whole_api_claims() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let legacy = service(&f, false).await;
    let native = service(&f, true).await;
    let cookie = cookie("owned-a-token");
    for service in [&legacy, &native] {
        for _ in 0..20 {
            assert!(
                service
                    .authenticate_cookie(&cookie)
                    .await
                    .unwrap()
                    .is_some()
            );
        }
    }
    let mut a = Vec::new();
    let mut b = Vec::new();
    for round in 0..4 {
        for offset in 0..2 {
            let (service, samples) = if (round + offset) % 2 == 0 {
                (&legacy, &mut a)
            } else {
                (&native, &mut b)
            };
            for _ in 0..50 {
                let start = std::time::Instant::now();
                assert!(
                    service
                        .authenticate_cookie(&cookie)
                        .await
                        .unwrap()
                        .is_some()
                );
                samples.push(start.elapsed().as_micros());
            }
        }
    }
    for (driver, samples) in [("legacy", &mut a), ("native", &mut b)] {
        samples.sort_unstable();
        println!(
            "session_benchmark driver={driver} samples={} p50_us={} p95_us={} p99_us={}",
            samples.len(),
            samples[samples.len() / 2],
            samples[samples.len() * 95 / 100],
            samples[samples.len() * 99 / 100]
        );
    }
}

#[tokio::test]
#[ignore = "owned local PostgreSQL18; cancellation of an actual native session query"]
async fn cancelled_native_session_read_discards_its_connection() {
    use sea_orm::TransactionTrait;
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let reads = pool(&f.bootstrap_url, "webdock_auth_bootstrap").await;
    let mut connection = reads.acquire().await.unwrap();
    let tx = connection.transaction().await.unwrap();
    let original: i32 = tx
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    tx.rollback().await.unwrap();
    drop(connection);
    let native = std::sync::Arc::new(
        IdentityService::new(
            Database::connect(&f.bootstrap_url).await.unwrap(),
            CookiePolicy::new(false, AUTH_SECRET).unwrap(),
        )
        .with_native_session_reads(reads.clone()),
    );
    let gate = f.owner.begin().await.unwrap();
    sql(
        &gate,
        "LOCK TABLE webdock_auth.session IN ACCESS EXCLUSIVE MODE",
    )
    .await;
    let request =
        tokio::spawn(async move { native.authenticate_cookie(&cookie("owned-a-token")).await });
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let row=f.owner.query_one_raw(sea_orm::Statement::from_sql_and_values(sea_orm::DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND wait_event_type='Lock' AND query LIKE '%native_provider_session_live%') AS blocked",[original.into()])).await.unwrap().unwrap();
        if row.try_get::<bool>("", "blocked").unwrap() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "owned native read must be in flight before cancellation"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    assert_eq!(
        reads.status().size,
        0,
        "cancelled native session query cannot return its connection to the pool"
    );
    gate.commit().await.unwrap();
    let mut connection = reads.acquire().await.unwrap();
    let tx = connection.transaction().await.unwrap();
    let replacement: i32 = tx
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    assert_ne!(replacement, original);
    tx.rollback().await.unwrap();
}

#[tokio::test]
#[ignore = "owned local PostgreSQL18; real local MFA/passkey flows with native session reads"]
async fn native_reads_interoperate_with_operator_mfa_and_passkeys() {
    operator_security_flow(false).await;
}
#[tokio::test]
#[ignore = "owned local PostgreSQL18; MFA/passkey flow with native session management"]
async fn native_management_interoperates_with_operator_mfa_and_passkeys() {
    operator_security_flow(true).await;
}
async fn operator_security_flow(management: bool) {
    use webauthn_authenticator_rs::{WebauthnAuthenticator, softpasskey::SoftPasskey};
    use webdock_identity::{MfaProof, PasswordAuthentication, SecretKeys};
    let f = fixture::Fixture::start().await;
    let origin = url::Url::parse("https://auth.example.test").unwrap();
    let service = service(&f, true).await;
    let service = if management {
        service
            .with_native_session_management(pool(&f.bootstrap_url, "webdock_auth_bootstrap").await)
    } else {
        service
    };
    let service = service
        .with_allocator(std::sync::Arc::new(
            webdock_persistence::IdAllocator::connect_native(
                &f.bootstrap_url,
                webdock_ids::Identity::platform(0, 2).unwrap(),
                "webdock_auth_bootstrap",
                false,
            )
            .await
            .unwrap(),
        ))
        .with_secret_keys(
            SecretKeys::new(
                std::collections::BTreeMap::from([(1, AUTH_SECRET.into())]),
                1,
                Some(AUTH_SECRET.into()),
            )
            .unwrap(),
        )
        .with_passkeys(origin.clone())
        .unwrap();
    let email = "native-security@example.test";
    let password = "owned-native-security-password-123";
    let user = service
        .create_local_account(email, "Native operator", password, true)
        .await
        .unwrap();
    f.owner.execute_raw(sea_orm::Statement::from_sql_and_values(sea_orm::DbBackend::Postgres,"UPDATE webdock_auth.\"user\" SET \"emailVerified\"=true,\"mustChangePassword\"=false WHERE id=$1",[user.into()])).await.unwrap();
    let PasswordAuthentication::SetupRequired {
        session: Some(setup),
        ..
    } = service
        .authenticate_password(email, password)
        .await
        .unwrap()
    else {
        panic!("operator must complete MFA setup");
    };
    assert!(!setup.principal.can_access());
    let session_cookie = |value: &str| value.split(';').next().unwrap().to_owned();
    let enrollment = service
        .begin_totp_enrollment(&session_cookie(&setup.set_cookie), password)
        .await
        .unwrap();
    let totp = totp_rs::Totp::from_url(&enrollment.totp_uri).unwrap();
    let enrolled = service
        .confirm_totp_enrollment(
            &session_cookie(&setup.set_cookie),
            &totp.generate_current().to_string(),
        )
        .await
        .unwrap();
    assert!(enrolled.principal.is_operator());
    let PasswordAuthentication::MfaRequired {
        set_cookie: challenge,
        ..
    } = service
        .authenticate_password(email, password)
        .await
        .unwrap()
    else {
        panic!("first factor cannot bypass MFA");
    };
    assert!(
        service
            .authenticate_cookie(&session_cookie(&challenge))
            .await
            .unwrap()
            .is_none()
    );
    let verified = service
        .complete_mfa(
            &session_cookie(&challenge),
            MfaProof::RecoveryCode(enrollment.backup_codes[0].clone()),
        )
        .await
        .unwrap();
    assert!(verified.principal.is_operator());
    let registration = service
        .begin_passkey_registration(&session_cookie(&verified.set_cookie))
        .await
        .unwrap();
    let mut options = registration.options;
    let selection = options.public_key.authenticator_selection.as_mut().unwrap();
    assert!(selection.require_resident_key);
    assert_eq!(
        selection.user_verification,
        webauthn_rs_core::proto::UserVerificationPolicy::Required
    );
    // Existing software-authenticator test exception: it cannot store resident keys.
    // Change client selection only; server UV/signature/origin verification stays intact.
    selection.require_resident_key = false;
    selection.resident_key = None;
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let response = authenticator
        .do_registration(origin.clone(), options)
        .unwrap();
    let credential = response.raw_id.clone();
    let combined = format!(
        "{}; {}",
        session_cookie(&verified.set_cookie),
        session_cookie(&registration.set_cookie)
    );
    service
        .finish_passkey_registration(&combined, &response, Some("Owned native test key"))
        .await
        .unwrap();
    let ceremony = service.begin_passkey_authentication().await.unwrap();
    let mut options = ceremony.options;
    options.public_key.allow_credentials = vec![webauthn_rs_core::proto::AllowCredentials {
        type_: "public-key".into(),
        id: credential,
        transports: None,
    }];
    options.mediation = None;
    let assertion = authenticator.do_authentication(origin, options).unwrap();
    let signed = service
        .finish_passkey_authentication(&session_cookie(&ceremony.set_cookie), &assertion)
        .await
        .unwrap();
    assert!(signed.principal.is_operator());
    assert!(
        service
            .finish_passkey_authentication(&session_cookie(&ceremony.set_cookie), &assertion)
            .await
            .is_err(),
        "passkey ceremony remains single use"
    );
    assert!(
        service
            .revoke_owned_session(
                &session_cookie(&verified.set_cookie),
                &signed.principal.session_id
            )
            .await
            .unwrap()
    );
    assert!(
        service
            .authenticate_cookie(&session_cookie(&signed.set_cookie))
            .await
            .unwrap()
            .is_none()
    );
}
