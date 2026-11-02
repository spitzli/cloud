//! Invitation delivery uses the authority admitted to its outbox job, never restored operator rights.
use sea_orm::{ConnectionTrait, Database, DbBackend, Statement, TransactionTrait};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use webdock_access::{AccessConfig, AccessService, MemberRole, PreviewMode, TenantCommand};
use webdock_identity::{
    CookiePolicy, Delivery, DeliveryError, DeliveryRequest, IdentityService, RlsSigner, SecretKeys,
};

#[derive(Default)]
struct Sink(Mutex<Vec<DeliveryRequest>>);
#[async_trait::async_trait]
impl Delivery for Sink {
    async fn deliver(&self, request: &DeliveryRequest) -> Result<(), DeliveryError> {
        self.0.lock().unwrap().push(request.clone());
        Ok(())
    }
}
fn guarded(key: &str) -> url::Url {
    let raw = std::env::var(key).expect("explicit owned fixture configuration");
    let url = url::Url::parse(&raw).unwrap();
    assert!(["postgres", "postgresql"].contains(&url.scheme()));
    assert!([Some("localhost"), Some("127.0.0.1")].contains(&url.host_str()));
    assert_eq!(url.path(), "/webdock_rls_test");
    assert!(url.query().is_none() && url.fragment().is_none());
    url
}
async fn sql(db: &impl ConnectionTrait, query: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        query,
        values,
    ))
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "creates and removes an isolated database on the owned restricted fixture server"]
async fn invitation_dispatch_binds_live_customer_owner_scope() {
    let mut offline = guarded("WEBDOCK_TEST_MIGRATION_DATABASE_URL");
    let admin = Database::connect(offline.as_str()).await.unwrap();
    let name = format!(
        "webdock_access_invite_{}_{}_test",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    );
    admin
        .execute_unprepared(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    offline.set_path(&format!("/{name}"));
    let owned_name = name.clone();
    let result = tokio::spawn(async move { exercise(offline, owned_name).await }).await;
    admin
        .execute_unprepared(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .await
        .unwrap();
    admin.close().await.unwrap();
    if let Err(error) = result {
        std::panic::resume_unwind(error.into_panic());
    }
}

async fn exercise(offline: url::Url, name: String) {
    let owner = Database::connect(offline.as_str()).await.unwrap();
    let rls_secret = std::env::var("DATABASE_RLS_SECRET").unwrap();
    webdock_persistence::migrations::bootstrap(&owner, &name, &rls_secret)
        .await
        .unwrap();
    let mut auth_url = guarded("AUTH_DATABASE_URL");
    auth_url.set_path(&format!("/{name}"));
    let mut business_url = guarded("DATABASE_URL");
    business_url.set_path(&format!("/{name}"));
    let auth = Database::connect(auth_url.as_str()).await.unwrap();
    let business = Database::connect(business_url.as_str()).await.unwrap();
    let ids = Arc::new(
        webdock_persistence::IdAllocator::connect(
            auth_url.as_str(),
            webdock_ids::Identity::platform(0, 2).unwrap(),
        )
        .await
        .unwrap(),
    );
    let secret = std::env::var("BETTER_AUTH_SECRET").unwrap();
    let identity = Arc::new(
        IdentityService::new(
            auth.clone(),
            CookiePolicy::new(false, secret.clone()).unwrap(),
        )
        .with_allocator(ids.clone()),
    );
    let sink = Arc::new(Sink::default());
    let access = AccessService::new(
        business.clone(),
        auth.clone(),
        identity.clone(),
        ids.clone(),
        RlsSigner::new(rls_secret, "webdock_auth_runtime").unwrap(),
        Arc::new(SecretKeys::new(BTreeMap::from([(0, secret.clone())]), 0, Some(secret)).unwrap()),
        AccessConfig {
            auth_origin: url::Url::parse("https://auth.example.invalid").unwrap(),
            console_origins: vec![],
            allow_local_http: false,
        },
    )
    .unwrap()
    .with_delivery(sink.clone());
    let customer = ids.allocate().await.unwrap().to_string();
    let org = ids.allocate().await.unwrap().to_string();
    sql(&owner,"WITH hint AS MATERIALIZED (SELECT set_config('webdock.organization_allocation',jsonb_build_object('customer_id',$1::text,'organization_id',$2::text)::text,true)) INSERT INTO webdock_admin.customers(id,name) SELECT $1,'Invitation scope fixture' FROM hint",vec![customer.clone().into(),org.clone().into()]).await;
    for scenario in [
        "ordinary-owner",
        "ordinary-operator",
        "live-acting",
        "read-only",
        "exit",
        "expiry",
        "demotion",
        "revocation",
        "reentry",
        "organization-authority",
        "customer-inactive",
        "membership-loss",
        "legacy-unbound",
    ] {
        let user = format!("inviter-{scenario}");
        let session = format!("session-{scenario}");
        let recipient = format!("recipient-{scenario}");
        let email = format!("{recipient}@example.invalid");
        let ordinary_owner = ["ordinary-owner", "membership-loss"].contains(&scenario);
        let role = if ordinary_owner { "user" } else { "operator" };
        sql(&owner,"INSERT INTO webdock_auth.\"user\"(id,name,email,role,\"emailVerified\",\"twoFactorEnabled\",\"mustChangePassword\") VALUES($1,$1,$2,$3,true,true,false),($4,$4,$5,'user',true,false,false)",vec![user.clone().into(),format!("{user}@example.invalid").into(),role.into(),recipient.clone().into(),email.clone().into()]).await;
        sql(&owner,"INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES($1,$1,$2,now()+interval '1 hour',now())",vec![session.clone().into(),user.clone().into()]).await;
        if ordinary_owner {
            sql(&owner,"INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES($1,$2,$3,'owner',now())",vec![format!("member-{scenario}").into(),org.clone().into(),user.clone().into()]).await;
        }
        let actor = identity
            .authenticate_session_in(&auth.begin().await.unwrap(), &user, &session)
            .await
            .unwrap()
            .unwrap();
        let scoped = ![
            "ordinary-owner",
            "ordinary-operator",
            "membership-loss",
            "legacy-unbound",
        ]
        .contains(&scenario);
        if scoped {
            access
                .start_customer_scope_as(
                    &actor,
                    &customer,
                    if scenario == "read-only" {
                        PreviewMode::ReadOnly
                    } else {
                        PreviewMode::Owner
                    },
                )
                .await
                .unwrap();
        }
        let current = identity
            .authenticate_session_in(&auth.begin().await.unwrap(), &user, &session)
            .await
            .unwrap()
            .unwrap();
        let result = access
            .manage_tenant_as(
                &current,
                &customer,
                TenantCommand::Invite {
                    email: email.clone(),
                    name: recipient.clone(),
                    role: MemberRole::Member,
                },
            )
            .await;
        if scenario == "read-only" {
            assert!(result.is_err());
            let queued=owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT count(*) AS count FROM webdock_auth.access_delivery_job WHERE actor_id=$1",[user.clone().into()])).await.unwrap().unwrap();
            assert_eq!(
                queued.try_get::<i64>("", "count").unwrap(),
                0,
                "read-only admission queues no work"
            );
            continue;
        }
        let invitation = result.unwrap().id.unwrap();
        let admitted=owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT admission_bound,admitted_customer_scope,actor_id,actor_session_id FROM webdock_auth.access_delivery_job WHERE invitation_id=$1",[invitation.clone().into()])).await.unwrap().unwrap();
        assert!(admitted.try_get::<bool>("", "admission_bound").unwrap());
        assert_eq!(
            admitted
                .try_get::<Option<String>>("", "admitted_customer_scope")
                .unwrap()
                .as_deref(),
            current.customer_scope()
        );
        assert_eq!(admitted.try_get::<String>("", "actor_id").unwrap(), user);
        assert_eq!(
            admitted.try_get::<String>("", "actor_session_id").unwrap(),
            session
        );
        if scenario == "live-acting" {
            let proof = RlsSigner::new(
                std::env::var("DATABASE_RLS_SECRET").unwrap(),
                "webdock_auth_runtime",
            )
            .unwrap()
            .begin(&business, &current)
            .await
            .unwrap();
            assert!(proof.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.access_delivery_job SET admitted_customer_scope=NULL WHERE invitation_id=$1",[invitation.clone().into()])).await.is_err(),"runtime cannot erase admitted acting scope");
            proof.rollback().await.unwrap();
        }
        if scenario == "legacy-unbound" {
            sql(&owner,"UPDATE webdock_auth.access_delivery_job SET admission_bound=false WHERE invitation_id=$1",vec![invitation.clone().into()]).await;
        }
        match scenario {
            "exit" => {
                access.exit_preview_as(&current).await.unwrap();
            }
            "expiry" => {
                sql(&owner,"UPDATE webdock_auth.studio_tenant_preview SET expires_at=now()-interval '1 second' WHERE session_id=$1",vec![session.clone().into()]).await;
            }
            "demotion" => {
                sql(
                    &owner,
                    "UPDATE webdock_auth.\"user\" SET role='user' WHERE id=$1",
                    vec![user.clone().into()],
                )
                .await;
            }
            "revocation" => {
                sql(
                    &owner,
                    "DELETE FROM webdock_auth.session WHERE id=$1",
                    vec![session.clone().into()],
                )
                .await;
            }
            "reentry" => {
                access.exit_preview_as(&current).await.unwrap();
                access
                    .start_customer_scope_as(&actor, &customer, PreviewMode::Owner)
                    .await
                    .unwrap();
            }
            "customer-inactive" => {
                sql(
                    &owner,
                    "UPDATE webdock_admin.customers SET status='archived' WHERE id=$1",
                    vec![customer.clone().into()],
                )
                .await;
            }
            "organization-authority" => {
                sql(&owner,"INSERT INTO webdock_auth.clerk_organization(issuer,clerk_organization_id,organization_id) VALUES('https://invitation-fixture.invalid','scoped',$1)",vec![org.clone().into()]).await;
            }
            "membership-loss" => {
                sql(
                    &owner,
                    "UPDATE webdock_auth.member SET role='member' WHERE id=$1",
                    vec![format!("member-{scenario}").into()],
                )
                .await;
            }
            _ => {}
        }
        let expected = ["ordinary-owner", "ordinary-operator", "live-acting"].contains(&scenario);
        let before = sink.0.lock().unwrap().len();
        let batch = access.dispatch_invitation_jobs(100).await.unwrap();
        assert_eq!(
            batch.delivered,
            u32::from(expected),
            "{scenario}: delivered"
        );
        assert_eq!(batch.canceled, u32::from(!expected), "{scenario}: canceled");
        assert_eq!(batch.failed, 0, "{scenario}: failed");
        assert_eq!(
            sink.0.lock().unwrap().len() - before,
            usize::from(expected),
            "{scenario}: sink"
        );
        if expected {
            assert_eq!(sink.0.lock().unwrap().last().unwrap().email, email);
        }
        sql(
            &owner,
            "UPDATE webdock_admin.customers SET status='active' WHERE id=$1",
            vec![customer.clone().into()],
        )
        .await;
        sql(&owner,"DELETE FROM webdock_auth.clerk_organization WHERE issuer='https://invitation-fixture.invalid'",vec![]).await;
        sql(
            &owner,
            "UPDATE webdock_auth.invitation SET status='canceled' WHERE id=$1",
            vec![invitation.into()],
        )
        .await;
    }
    drop(access);
    drop(identity);
    drop(ids);
    auth.close().await.unwrap();
    business.close().await.unwrap();
    owner.close().await.unwrap();
}
