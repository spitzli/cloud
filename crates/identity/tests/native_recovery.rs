//! Actual restricted PostgreSQL recovery; the retained DB is deliberately disconnected.
#[path = "support/native_pg.rs"]
mod fixture;
use fixture::AUTH_SECRET;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use spitzli_identity::{
    CookiePolicy, Delivery, DeliveryError, DeliveryKind, DeliveryRequest, IdentityError,
    IdentityService, SecretKeys,
};
use spitzli_persistence::{
    IdAllocator,
    postgres::{Limits, Pool},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct Capture(Mutex<Vec<DeliveryRequest>>);
#[async_trait::async_trait]
impl Delivery for Capture {
    async fn deliver(&self, request: &DeliveryRequest) -> Result<(), DeliveryError> {
        self.0.lock().unwrap().push(request.clone());
        Ok(())
    }
}
async fn service(f: &fixture::Fixture, capture: Arc<dyn Delivery>) -> IdentityService {
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
    IdentityService::new(
        DatabaseConnection::default(),
        CookiePolicy::new(false, AUTH_SECRET).unwrap(),
    )
    .with_native_account_recovery(pool)
    .with_allocator(Arc::new(
        IdAllocator::connect_native(
            &f.bootstrap_url,
            spitzli_ids::Identity::platform(0, 2).unwrap(),
            "webdock_auth_bootstrap",
            false,
        )
        .await
        .unwrap(),
    ))
    .with_secret_keys(
        SecretKeys::new(
            BTreeMap::from([(0, AUTH_SECRET.into())]),
            0,
            Some(AUTH_SECRET.into()),
        )
        .unwrap(),
    )
    .with_account_origin(url::Url::parse("http://localhost:3125").unwrap())
    .unwrap()
    .with_delivery(capture)
}
async fn sql(f: &fixture::Fixture, sql: &str) {
    f.owner.execute_unprepared(sql).await.unwrap();
}
async fn seed(f: &fixture::Fixture) {
    let hash = spitzli_identity::hash_password("Owned-original-password-123").unwrap();
    sql(f,r#"INSERT INTO webdock_auth."user"(id,name,email,"emailVerified",role,"twoFactorEnabled","mustChangePassword") VALUES('a','A','a@example.test',false,'user',true,true),('b','B','b@example.test',true,'user',false,false);
    INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES('11','owned-a-token','a',now()+interval '1 hour',now()),('12','owned-b-token','b',now()+interval '1 hour',now());
    INSERT INTO webdock_auth.identity_mfa_challenge(token_hash,user_id,credential_digest,expires_at) VALUES(repeat('a',64),'a','old',now()+interval '1 hour')"#).await;
    f.owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,r#"INSERT INTO webdock_auth.account(id,"accountId","providerId","userId",password,"updatedAt") VALUES('21','a','credential','a',$1,now()),('22','b','credential','b',$1,now())"#,[hash.into()])).await.unwrap();
}
async fn count(f: &fixture::Fixture, table: &str) -> i64 {
    // Test-owned literal table names only.
    f.owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            format!("SELECT count(*) AS n FROM webdock_auth.{table}"),
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "n")
        .unwrap()
}
fn token(link: &str) -> String {
    url::Url::parse(link)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "token")
        .unwrap()
        .1
        .into_owned()
}

#[tokio::test]
#[ignore = "owned PostgreSQL18; actual activation/delivery/reset without retained DB"]
async fn native_activation_delivers_encrypted_one_use_proof_and_retires_only_its_sessions() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let capture = Arc::new(Capture::default());
    let service = service(&f, capture.clone()).await;
    let link = service
        .queue_account_activation("a", Some("owned-invitation"))
        .await
        .unwrap()
        .unwrap();
    assert!(link.starts_with("http://localhost:3125/reset-password?token="));
    assert!(link.ends_with("&invitation=owned-invitation"));
    assert!(
        service
            .queue_account_activation("a", None)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(count(&f, "identity_recovery_token").await, 1);
    assert_eq!(count(&f, "identity_delivery_job").await, 1);
    let row = f
        .owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT token_hash,payload FROM webdock_auth.identity_delivery_job",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_ne!(
        row.try_get::<String>("", "token_hash").unwrap(),
        token(&link)
    );
    assert!(
        !row.try_get::<String>("", "payload")
            .unwrap()
            .contains(&token(&link))
    );
    assert_eq!(
        service.dispatch_delivery_jobs(10).await.unwrap().delivered,
        1
    );
    assert_eq!(
        service.dispatch_delivery_jobs(10).await.unwrap().delivered,
        0
    );
    let sent = capture.0.lock().unwrap()[0].clone();
    assert_eq!(sent.email, "a@example.test");
    assert!(sent.kind == DeliveryKind::Activation);
    assert_eq!(sent.url, link);
    assert_eq!(
        service
            .complete_password_reset(&token(&link), "Owned-new-password-456")
            .await
            .unwrap(),
        "a@example.test"
    );
    assert!(matches!(
        service
            .complete_password_reset(&token(&link), "Owned-other-password-789")
            .await,
        Err(IdentityError::Unauthorized)
    ));
    assert_eq!(count(&f, "identity_recovery_token").await, 0);
    assert_eq!(count(&f, "identity_delivery_job").await, 0);
    assert_eq!(count(&f, "identity_mfa_challenge").await, 0);
    let row=f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,r#"SELECT a.password,u."twoFactorEnabled",u."mustChangePassword",u."emailVerified" FROM webdock_auth.account a JOIN webdock_auth."user" u ON u.id=a."userId" WHERE u.id='a'"#)).await.unwrap().unwrap();
    assert!(
        spitzli_identity::verify_password(
            "Owned-new-password-456",
            &row.try_get::<String>("", "password").unwrap()
        )
        .unwrap()
    );
    assert!(row.try_get::<bool>("", "twoFactorEnabled").unwrap());
    assert!(row.try_get::<bool>("", "emailVerified").unwrap());
    assert!(!row.try_get::<bool>("", "mustChangePassword").unwrap());
    let row = f
        .owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT id FROM webdock_auth.session",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.try_get::<String>("", "id").unwrap(), "12");
}

#[tokio::test]
#[ignore = "owned PostgreSQL18; public acknowledgement and request throttle"]
async fn native_reset_normalizes_email_and_never_queues_for_unknown_or_banned_accounts() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let service = service(&f, Arc::new(Capture::default())).await;
    service
        .request_password_reset("missing@example.test")
        .await
        .unwrap();
    service
        .request_password_reset(&"x".repeat(255))
        .await
        .unwrap();
    assert_eq!(count(&f, "identity_recovery_token").await, 0);
    sql(
        &f,
        r#"UPDATE webdock_auth."user" SET banned=true WHERE id='b'"#,
    )
    .await;
    service
        .request_password_reset("b@example.test")
        .await
        .unwrap();
    assert_eq!(count(&f, "identity_recovery_token").await, 0);
    let (a, b) = tokio::join!(
        service.request_password_reset(" A@EXAMPLE.TEST "),
        service.request_password_reset("a@example.test")
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(count(&f, "identity_recovery_token").await, 1);
    assert_eq!(count(&f, "identity_delivery_job").await, 1);
}

#[tokio::test]
#[ignore = "owned PostgreSQL18; current identity/credential/provider proof refuses stale recovery"]
async fn native_reset_rejects_changed_identity_credential_expiry_and_managed_bindings() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let service = service(&f, Arc::new(Capture::default())).await;
    let link = service
        .queue_account_activation("a", None)
        .await
        .unwrap()
        .unwrap();
    for (change, restore) in [
        (
            r#"UPDATE webdock_auth."user" SET email='changed@example.test' WHERE id='a'"#,
            r#"UPDATE webdock_auth."user" SET email='a@example.test' WHERE id='a'"#,
        ),
        (
            r#"UPDATE webdock_auth."user" SET banned=true WHERE id='a'"#,
            r#"UPDATE webdock_auth."user" SET banned=false WHERE id='a'"#,
        ),
        (
            "UPDATE webdock_auth.identity_recovery_token SET credential_digest='wrong'",
            "UPDATE webdock_auth.identity_recovery_token SET credential_digest=encode(sha256(convert_to((SELECT password FROM webdock_auth.account WHERE id='21'),'UTF8')),'hex')",
        ),
        (
            "UPDATE webdock_auth.identity_recovery_token SET expires_at=now()-interval '1 second'",
            "UPDATE webdock_auth.identity_recovery_token SET expires_at=now()+interval '1 hour'",
        ),
    ] {
        sql(&f, change).await;
        assert!(matches!(
            service
                .complete_password_reset(&token(&link), "Owned-must-not-change-456")
                .await,
            Err(IdentityError::Unauthorized)
        ));
        assert_eq!(count(&f, "session").await, 2);
        assert_eq!(count(&f, "identity_recovery_token").await, 1);
        sql(&f, restore).await;
    }
    sql(&f,"INSERT INTO webdock_auth.neon_identity(user_id,provider_origin,provider_subject,enabled,linked_by) VALUES('a','https://ep-owned.neonauth.eu.neon.tech/neondb/auth','00000000-0000-0000-0000-000000000001',false,'owned')").await;
    assert!(matches!(
        service
            .complete_password_reset(&token(&link), "Owned-must-not-change-456")
            .await,
        Err(IdentityError::Unauthorized)
    ));
    assert!(matches!(
        service.queue_account_activation("a", None).await,
        Err(IdentityError::SecurityState)
    ));
    assert!(
        matches!(
            service.request_password_reset("a@example.test").await,
            Err(IdentityError::Configuration)
        ),
        "missing provider must not fall back to local recovery"
    );
    assert_eq!(count(&f, "identity_recovery_token").await, 1);
    assert_eq!(count(&f, "session").await, 2);
}

#[tokio::test]
#[ignore = "owned PostgreSQL18; failed commit cannot consume proof or report delivery"]
async fn native_recovery_never_reports_a_failed_commit_as_success() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let capture = Arc::new(Capture::default());
    let service = service(&f, capture.clone()).await;
    sql(&f,"CREATE FUNCTION webdock_auth.owned_recovery_commit_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'owned deferred failure'; END $$;CREATE CONSTRAINT TRIGGER owned_recovery_commit_failure AFTER INSERT ON webdock_auth.identity_delivery_job DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION webdock_auth.owned_recovery_commit_failure()").await;
    assert!(matches!(
        service.queue_account_activation("a", None).await,
        Err(IdentityError::Persistence)
    ));
    assert_eq!(count(&f, "identity_recovery_token").await, 0);
    assert_eq!(count(&f, "identity_delivery_job").await, 0);
    sql(
        &f,
        "DROP TRIGGER owned_recovery_commit_failure ON webdock_auth.identity_delivery_job",
    )
    .await;
    let link = service
        .queue_account_activation("a", None)
        .await
        .unwrap()
        .unwrap();
    sql(&f,"CREATE CONSTRAINT TRIGGER owned_recovery_commit_failure AFTER UPDATE ON webdock_auth.identity_delivery_job DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION webdock_auth.owned_recovery_commit_failure()").await;
    assert!(matches!(
        service.dispatch_delivery_jobs(1).await,
        Err(IdentityError::Persistence)
    ));
    assert!(
        capture.0.lock().unwrap().is_empty(),
        "failed claim commit must not deliver"
    );
    sql(&f,"DROP TRIGGER owned_recovery_commit_failure ON webdock_auth.identity_delivery_job;CREATE CONSTRAINT TRIGGER owned_recovery_commit_failure AFTER UPDATE ON webdock_auth.account DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION webdock_auth.owned_recovery_commit_failure()").await;
    assert!(matches!(
        service
            .complete_password_reset(&token(&link), "Owned-new-password-456")
            .await,
        Err(IdentityError::Persistence)
    ));
    assert_eq!(count(&f, "session").await, 2);
    assert_eq!(count(&f, "identity_recovery_token").await, 1);
    assert_eq!(count(&f, "identity_mfa_challenge").await, 1);
    sql(&f,"DROP TRIGGER owned_recovery_commit_failure ON webdock_auth.account;DROP FUNCTION webdock_auth.owned_recovery_commit_failure()").await;
    assert_eq!(
        service
            .complete_password_reset(&token(&link), "Owned-new-password-456")
            .await
            .unwrap(),
        "a@example.test"
    );
}

#[derive(Default)]
struct GateDelivery {
    started: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl Delivery for GateDelivery {
    async fn deliver(&self, _: &DeliveryRequest) -> Result<(), DeliveryError> {
        self.started.notify_one();
        self.release.notified().await;
        Ok(())
    }
}
#[tokio::test]
#[ignore = "owned PostgreSQL18; delivery releases max1 pool and stale generation fails closed"]
async fn native_delivery_releases_connection_and_fences_its_receipt() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let sink = Arc::new(GateDelivery::default());
    let service = Arc::new(service(&f, sink.clone()).await);
    service.queue_account_activation("a", None).await.unwrap();
    let job = service.clone();
    let running = tokio::spawn(async move { job.dispatch_delivery_jobs(1).await });
    tokio::time::timeout(std::time::Duration::from_secs(3), sink.started.notified())
        .await
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        service.request_password_reset("missing@example.test"),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        service.dispatch_delivery_jobs(1).await.unwrap().delivered,
        0,
        "an active claim must not be redelivered"
    );
    sql(
        &f,
        "UPDATE webdock_auth.identity_delivery_job SET attempts=attempts+1",
    )
    .await;
    sink.release.notify_one();
    assert!(matches!(
        running.await.unwrap(),
        Err(IdentityError::Persistence)
    ));
    let row = f
        .owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT delivered_at IS NULL AS pending FROM webdock_auth.identity_delivery_job",
        ))
        .await
        .unwrap()
        .unwrap();
    assert!(row.try_get::<bool>("", "pending").unwrap());
}

#[tokio::test]
#[ignore = "owned PostgreSQL18; recovery expiry is current after credential lock waits"]
async fn native_reset_rechecks_expiry_after_waiting_for_credential_lock() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let service = Arc::new(service(&f, Arc::new(Capture::default())).await);
    let link = service
        .queue_account_activation("a", None)
        .await
        .unwrap()
        .unwrap();
    let guard = f.owner.begin().await.unwrap();
    guard
        .execute_unprepared("UPDATE webdock_auth.account SET password=password WHERE id='21'")
        .await
        .unwrap();
    sql(&f,"UPDATE webdock_auth.identity_recovery_token SET expires_at=statement_timestamp()+interval '3 seconds'").await;
    let job = service.clone();
    let running = tokio::spawn(async move {
        job.complete_password_reset(&token(&link), "Owned-must-not-change-456")
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2),async {
        loop {
            let row=f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name='spitzli-native-postgres' AND wait_event_type='Lock' AND query LIKE 'SELECT password%') AS blocked")).await.unwrap().unwrap();
            if row.try_get::<bool>("","blocked").unwrap(){break;}
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(4),async {
        loop {
            let row=f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT expires_at<clock_timestamp() AS expired FROM webdock_auth.identity_recovery_token")).await.unwrap().unwrap();
            if row.try_get::<bool>("","expired").unwrap(){break;}
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }).await.unwrap();
    guard.commit().await.unwrap();
    assert!(matches!(
        running.await.unwrap(),
        Err(IdentityError::Unauthorized)
    ));
    assert_eq!(count(&f, "session").await, 2);
    assert_eq!(count(&f, "identity_recovery_token").await, 1);
}

#[derive(Default)]
struct FlakyDelivery {
    calls: Mutex<Vec<String>>,
}
#[async_trait::async_trait]
impl Delivery for FlakyDelivery {
    async fn deliver(&self, request: &DeliveryRequest) -> Result<(), DeliveryError> {
        let mut calls = self.calls.lock().unwrap();
        calls.push(request.id.clone());
        if calls.len() == 1 {
            Err(DeliveryError::Unavailable)
        } else {
            Ok(())
        }
    }
}
#[tokio::test]
#[ignore = "owned PostgreSQL18; delivery retries preserve stable ID and enforce attempt limit"]
async fn native_delivery_retries_stable_id_and_stops_at_attempt_ceiling() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let sink = Arc::new(FlakyDelivery::default());
    let service = service(&f, sink.clone()).await;
    service.queue_account_activation("a", None).await.unwrap();
    assert_eq!(service.dispatch_delivery_jobs(1).await.unwrap().failed, 1);
    assert_eq!(
        service.dispatch_delivery_jobs(1).await.unwrap().delivered,
        0
    );
    sql(
        &f,
        "UPDATE webdock_auth.identity_delivery_job SET next_attempt_at=statement_timestamp()",
    )
    .await;
    assert_eq!(
        service.dispatch_delivery_jobs(1).await.unwrap().delivered,
        1
    );
    let calls = sink.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0], calls[1]);
    sql(&f,"UPDATE webdock_auth.identity_delivery_job SET attempts=5,delivered_at=NULL,lease_until=NULL,next_attempt_at=statement_timestamp()").await;
    assert_eq!(
        service.dispatch_delivery_jobs(1).await.unwrap().delivered,
        0
    );
    assert_eq!(sink.calls.lock().unwrap().len(), 2);
    for limit in [0, 101] {
        assert!(matches!(
            service.dispatch_delivery_jobs(limit).await,
            Err(IdentityError::InvalidInput)
        ));
    }
}

#[tokio::test]
#[ignore = "owned PostgreSQL18; canceled recovery discards transaction and releases pool"]
async fn native_canceled_recovery_never_admits_proof_or_poisoned_connection() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let service = Arc::new(service(&f, Arc::new(Capture::default())).await);
    let guard = f.owner.begin().await.unwrap();
    guard
        .execute_unprepared(r#"UPDATE webdock_auth."user" SET name=name WHERE id='a'"#)
        .await
        .unwrap();
    let job = service.clone();
    let running = tokio::spawn(async move { job.queue_account_activation("a", None).await });
    tokio::time::timeout(std::time::Duration::from_secs(2),async {
        loop {
            let row=f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name='spitzli-native-postgres' AND wait_event_type='Lock' AND query LIKE 'SELECT id FROM webdock_auth.%') AS blocked")).await.unwrap().unwrap();
            if row.try_get::<bool>("","blocked").unwrap(){break;}
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }).await.unwrap();
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    guard.rollback().await.unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        service.request_password_reset("missing@example.test"),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(count(&f, "identity_recovery_token").await, 0);
    assert_eq!(count(&f, "identity_delivery_job").await, 0);
    assert_eq!(count(&f, "session").await, 2);
}

#[tokio::test]
#[ignore = "owned PostgreSQL18; local migration authority must remain current after waits"]
async fn native_reset_refuses_local_authority_retired_during_credential_wait() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let service = Arc::new(service(&f, Arc::new(Capture::default())).await);
    let link = service
        .queue_account_activation("a", None)
        .await
        .unwrap()
        .unwrap();
    sql(&f,"INSERT INTO webdock_auth.clerk_identity(issuer,clerk_user_id,user_id,enabled) VALUES('https://fixture.invalid','legacy-a','a',false);INSERT INTO webdock_auth.local_identity_activation(user_id,bindings,approved_state,evidence_ref,evidence_sha256,operator_subject,archive_envelope) VALUES('a','[[\"legacy\",\"https://fixture.invalid\",\"legacy-a\"]]','{}','owned-evidence',repeat('a',64),'owned-operator','owned-archive')").await;
    let guard = f.owner.begin().await.unwrap();
    guard
        .execute_unprepared("UPDATE webdock_auth.account SET password=password WHERE id='21'")
        .await
        .unwrap();
    let job = service.clone();
    let running = tokio::spawn(async move {
        job.complete_password_reset(&token(&link), "Owned-must-not-change-456")
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2),async {
        loop {
            let row=f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name='spitzli-native-postgres' AND wait_event_type='Lock' AND query LIKE 'SELECT password%') AS blocked")).await.unwrap().unwrap();
            if row.try_get::<bool>("","blocked").unwrap(){break;}
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }).await.unwrap();
    sql(
        &f,
        "DELETE FROM webdock_auth.local_identity_activation WHERE user_id='a'",
    )
    .await;
    guard.commit().await.unwrap();
    assert!(matches!(
        running.await.unwrap(),
        Err(IdentityError::Unauthorized)
    ));
    assert_eq!(count(&f, "session").await, 2);
    assert_eq!(count(&f, "identity_recovery_token").await, 1);
}
