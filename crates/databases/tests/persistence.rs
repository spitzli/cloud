use sea_orm::{ConnectionTrait, Database, DbBackend, Statement, TransactionTrait};
use serde_json::json;
use spitzli_databases::{DatabaseService, StoredAuthority};
use spitzli_identity::{CookiePolicy, IdentityService, PasswordAuthentication, SecretKeys};
use std::{collections::BTreeMap, sync::Arc};
fn guarded(key: &str) -> String {
    let raw = std::env::var(key).unwrap();
    let u = url::Url::parse(&raw).unwrap();
    assert!(["postgres", "postgresql"].contains(&u.scheme()));
    assert!([Some("localhost"), Some("127.0.0.1"), Some("[::1]")].contains(&u.host_str()));
    assert_eq!(u.path(), "/webdock_rls_test");
    assert!(u.query().is_none() && u.fragment().is_none());
    raw
}
async fn run(db: &sea_orm::DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap();
}
#[tokio::test]
#[ignore = "owned retained-schema restricted-role fixture only"]
async fn persisted_launch_rechecks_grants_membership_and_session() {
    let owner = Database::connect(guarded("WEBDOCK_TEST_MIGRATION_DATABASE_URL"))
        .await
        .unwrap();
    // Offline native migrations, including 0014, are installed before this restricted fixture test.
    let auth_url = guarded("AUTH_DATABASE_URL");
    let auth = Database::connect(auth_url.clone()).await.unwrap();
    let db = Database::connect(guarded("DATABASE_URL")).await.unwrap();
    let worker = Database::connect(guarded("WEBDOCK_PLUGIN_WORKER_DATABASE_URL"))
        .await
        .unwrap();
    let ids = Arc::new(
        spitzli_persistence::IdAllocator::connect(
            &auth_url,
            spitzli_ids::Identity::platform(0, 2).unwrap(),
        )
        .await
        .unwrap(),
    );
    let module_version = format!("databases-fixture-{}", ids.allocate().await.unwrap());
    let module_added=owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version) VALUES('databases','builtin','enabled','enabled',$1) ON CONFLICT DO NOTHING",[module_version.clone().into()])).await.unwrap().rows_affected()==1;
    let secret = std::env::var("BETTER_AUTH_SECRET").unwrap();
    let identity = Arc::new(
        IdentityService::new(
            auth.clone(),
            CookiePolicy::new(false, secret.clone()).unwrap(),
        )
        .with_secret_keys(
            SecretKeys::new(
                BTreeMap::from([(0, secret.clone())]),
                0,
                Some(secret.clone()),
            )
            .unwrap(),
        )
        .with_allocator(ids.clone()),
    );
    let cipher = spitzli_secrets::SecretCipher::new(&secret).unwrap();
    let managed_cluster = ids.allocate().await.unwrap().to_string();
    let hosting = spitzli_hosting::HostingService::new(
        db.clone(),
        auth.clone(),
        worker.clone(),
        std::env::var("DATABASE_RLS_SECRET").unwrap(),
        secret.clone(),
        ids.clone(),
    )
    .await
    .unwrap();
    let service = DatabaseService::new(
        db,
        auth.clone(),
        worker,
        identity.clone(),
        None,
        ids.clone(),
        std::env::var("DATABASE_RLS_SECRET").unwrap(),
        &secret,
        "http://127.0.0.1:3150",
    )
    .await
    .unwrap()
    .with_management(spitzli_databases::ManagementConfig {
        cluster_id: Some(managed_cluster.clone()),
        endpoint_domain: Some("database.example.invalid".into()),
        public_ordering: false,
        protected_database_id: None,
        lease_secret: Some(secret.clone()),
    })
    .unwrap();
    let user = identity
        .create_local_account(
            &format!("database-{}@example.invalid", ids.allocate().await.unwrap()),
            "Database fixture",
            "fixture-password-123",
            false,
        )
        .await
        .unwrap();
    run(&auth,"UPDATE webdock_auth.\"user\" SET \"emailVerified\"=true,\"mustChangePassword\"=false WHERE id=$1",vec![user.clone().into()]).await;
    let row = auth
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT email FROM webdock_auth.\"user\" WHERE id=$1",
            [user.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap();
    let email: String = row.try_get("", "email").unwrap();
    let PasswordAuthentication::Authenticated(session) = identity
        .authenticate_password(&email, "fixture-password-123")
        .await
        .unwrap()
    else {
        panic!("login")
    };
    let actor = identity
        .authenticate_cookie(session.set_cookie.split(';').next().unwrap())
        .await
        .unwrap()
        .unwrap();
    let customer = ids.allocate().await.unwrap().to_string();
    let project = ids.allocate().await.unwrap().to_string();
    let binding = ids.allocate().await.unwrap().to_string();
    let grant = ids.allocate().await.unwrap().to_string();
    let member = ids.allocate().await.unwrap().to_string();
    run(
        &owner,
        "WITH hint AS MATERIALIZED (SELECT set_config('webdock.organization_allocation',jsonb_build_object('customer_id',$1::text,'organization_id',$2::text)::text,true)) INSERT INTO webdock_admin.customers(id,name) SELECT $1,'Database fixture' FROM hint",
        vec![customer.clone().into(),ids.allocate().await.unwrap().to_string().into()],
    )
    .await;
    let row = owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
            [customer.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap();
    let org: String = row.try_get("", "organization_id").unwrap();
    run(&owner,"INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES($1,$2,$3,'admin',now())",vec![member.clone().into(),org.clone().into(),user.clone().into()]).await;
    run(
        &owner,
        "INSERT INTO webdock_admin.projects(id,customer_id,name) VALUES($1,$2,'Database fixture')",
        vec![project.clone().into(), customer.clone().into()],
    )
    .await;
    run(&owner,"INSERT INTO webdock_auth.database_binding(id,project_id,name,environment) VALUES($1,$2,'Fixture','production')",vec![binding.clone().into(),project.clone().into()]).await;
    let runtime = format!("https://runtime-{binding}.invalid");
    let sealed = cipher
        .seal(
            &"fixture-proxy-secret-1234567890123456789",
            &format!("database:{binding}:{user}"),
        )
        .unwrap();
    run(&owner,"INSERT INTO webdock_auth.database_grant(id,binding_id,subject,profile,runtime_origin,connection_id,secret_sealed) VALUES($1,$2,$3,'read',$4,'fixture-connection',$5)",vec![grant.clone().into(),binding.clone().into(),user.clone().into(),runtime.clone().into(),sealed.into()]).await;
    run(&owner,"INSERT INTO webdock_auth.database_runtime_owner(runtime_origin,binding_id,subject,profile) VALUES($1,$2,$3,'read')",vec![runtime.clone().into(),binding.clone().into(),user.clone().into()]).await;

    let operator_email = format!(
        "database-operator-{}@example.invalid",
        ids.allocate().await.unwrap()
    );
    let operator_id = identity
        .create_local_account(
            &operator_email,
            "Database operator",
            "operator-fixture-123",
            true,
        )
        .await
        .unwrap();
    run(
        &auth,
        "UPDATE webdock_auth.\"user\" SET \"emailVerified\"=true WHERE id=$1",
        vec![operator_id.clone().into()],
    )
    .await;
    let PasswordAuthentication::SetupRequired {
        session: Some(setup),
        ..
    } = identity
        .authenticate_password(&operator_email, "operator-fixture-123")
        .await
        .unwrap()
    else {
        panic!("setup")
    };
    let changed = identity
        .change_password(
            setup.set_cookie.split(';').next().unwrap(),
            "operator-fixture-123",
            "operator-fixture-456",
        )
        .await
        .unwrap();
    let enroll = identity
        .begin_totp_enrollment(
            changed.set_cookie.split(';').next().unwrap(),
            "operator-fixture-456",
        )
        .await
        .unwrap();
    let totp = totp_rs::Totp::from_url(&enroll.totp_uri).unwrap();
    let completed = identity
        .confirm_totp_enrollment(
            changed.set_cookie.split(';').next().unwrap(),
            &totp.generate_current().to_string(),
        )
        .await
        .unwrap();
    let operator = identity
        .authenticate_cookie(completed.set_cookie.split(';').next().unwrap())
        .await
        .unwrap()
        .unwrap();
    let operator_mapping = ids.allocate().await.unwrap().to_string();
    run(&owner,"INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES($1,'Database operator',$2,'operator',$3)",vec![operator_mapping.clone().into(),operator_email.into(),operator_id.clone().into()]).await;
    let authority = StoredAuthority::LocalSession {};
    management_catalog(
        &owner,
        &service,
        &hosting,
        &operator,
        &actor,
        &authority,
        &customer,
        &project,
        &managed_cluster,
    )
    .await;
    managed_revisions(
        &owner, &service, &actor, &authority, &ids, &cipher, &customer, &project,
    )
    .await;

    assert!(service.execute(&actor,&authority,json!({"action":"create","projectID":project,"name":"Denied","environment":"production"})).await.is_err());

    let created=service.execute(&operator,&authority,json!({"action":"create","projectID":project,"name":"Operator fixture","environment":"staging"})).await.unwrap();
    let created_id = created["id"].as_str().unwrap().to_owned();
    assert!(
        service
            .execute(
                &operator,
                &authority,
                json!({"action":"open","bindingID":created_id})
            )
            .await
            .is_err(),
        "operator needs an explicit own grant"
    );
    let own_runtime = format!("https://own-{created_id}.invalid");
    let own_grant = json!({"action":"grant","bindingID":created_id,"subject":operator_id,"profile":"read","runtimeOrigin":own_runtime,"connectionID":"owned","proxySecret":"operator-proxy-fixture-123456789012345","isolationVerified":true,"databaseRoleVerified":true});
    service
        .execute(&operator, &authority, own_grant.clone())
        .await
        .unwrap();
    let mut changed = own_grant.clone();
    changed["profile"] = json!("schema");
    assert_eq!(
        service
            .execute(&operator, &authority, changed.clone())
            .await
            .unwrap_err()
            .status,
        409
    );
    changed["runtimeOrigin"] = json!(format!("https://own-next-{created_id}.invalid"));
    service
        .execute(&operator, &authority, changed)
        .await
        .unwrap();
    let mut reassigned = own_grant;
    reassigned["subject"] = json!(user);
    assert_eq!(
        service
            .execute(&operator, &authority, reassigned)
            .await
            .unwrap_err()
            .status,
        409,
        "retired origins cannot change owners"
    );
    run(&owner,"INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id) VALUES($1,$2,$3,$4)",vec![operator.session_id.clone().into(),operator_id.clone().into(),customer.clone().into(),org.clone().into()]).await;
    assert!(
        service
            .execute(
                &operator,
                &authority,
                json!({"action":"open","bindingID":created_id})
            )
            .await
            .is_err()
    );
    run(
        &owner,
        "DELETE FROM webdock_auth.studio_tenant_preview WHERE session_id=$1",
        vec![operator.session_id.clone().into()],
    )
    .await;
    run(&owner,"INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id,mode) VALUES($1,$2,$3,$4,'owner')",vec![operator.session_id.clone().into(),operator_id.clone().into(),customer.clone().into(),org.clone().into()]).await;
    let acting = identity
        .authenticate_session_in(
            &auth.begin().await.unwrap(),
            &operator.user_id,
            &operator.session_id,
        )
        .await
        .unwrap()
        .unwrap();
    let acting_authority = StoredAuthority::local_session(&acting);
    let launch = service
        .execute(
            &acting,
            &acting_authority,
            json!({"action":"open","bindingID":created_id}),
        )
        .await
        .unwrap();
    let scoped_session = service
        .exchange(launch["code"].as_str().unwrap())
        .await
        .unwrap();
    assert!(
        service
            .inspect(scoped_session["token"].as_str().unwrap())
            .await
            .is_ok()
    );
    run(
        &owner,
        "DELETE FROM webdock_auth.studio_tenant_preview WHERE session_id=$1",
        vec![operator.session_id.clone().into()],
    )
    .await;
    assert!(
        service
            .inspect(scoped_session["token"].as_str().unwrap())
            .await
            .is_err()
    );
    run(&owner,"INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id,mode) VALUES($1,$2,$3,$4,'owner')",vec![operator.session_id.clone().into(),operator_id.clone().into(),customer.clone().into(),org.clone().into()]).await;
    assert!(
        service
            .inspect(scoped_session["token"].as_str().unwrap())
            .await
            .is_err()
    );
    run(
        &owner,
        "DELETE FROM webdock_auth.studio_tenant_preview WHERE session_id=$1",
        vec![operator.session_id.clone().into()],
    )
    .await;
    service
        .execute(
            &operator,
            &authority,
            json!({"action":"disable","bindingID":created_id}),
        )
        .await
        .unwrap();
    for table in ["database_session", "database_launch"] {
        run(&owner,&format!("DELETE FROM webdock_auth.{table} WHERE grant_id IN (SELECT id FROM webdock_auth.database_grant WHERE binding_id=$1)"),vec![created_id.clone().into()]).await;
    }
    for table in ["database_audit", "database_runtime_owner", "database_grant"] {
        run(
            &owner,
            &format!("DELETE FROM webdock_auth.{table} WHERE binding_id=$1"),
            vec![created_id.clone().into()],
        )
        .await;
    }
    run(
        &owner,
        "DELETE FROM webdock_auth.database_binding WHERE id=$1",
        vec![created_id.into()],
    )
    .await;
    run(
        &auth,
        "DELETE FROM webdock_auth.\"user\" WHERE id=$1",
        vec![operator_id.into()],
    )
    .await;
    run(
        &owner,
        "DELETE FROM webdock_admin.users WHERE id=$1",
        vec![operator_mapping.into()],
    )
    .await;
    let directory = service
        .execute(
            &actor,
            &authority,
            json!({"action":"list","customerID":customer}),
        )
        .await
        .unwrap();
    assert_eq!(directory["bindings"].as_array().unwrap().len(), 1);
    let launch = service
        .execute(
            &actor,
            &authority,
            json!({"action":"open","bindingID":binding}),
        )
        .await
        .unwrap();
    let code = launch["code"].as_str().unwrap();
    let shape = owner.begin().await.unwrap();
    let invalid_shape = shape.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.database_launch SET native_authority='{}'::jsonb WHERE grant_id=$1",[grant.clone().into()])).await;
    shape.rollback().await.unwrap();
    assert!(
        invalid_shape.is_err(),
        "native proof rows require a non-null authority kind"
    );

    let session = service.exchange(code).await.unwrap();
    assert!(service.exchange(code).await.is_err());
    let token = session["token"].as_str().unwrap();
    let inspect = service.inspect(token).await.unwrap();
    assert_eq!(inspect["subject"], user);
    assert_eq!(inspect["profile"], "read");
    assert_eq!(
        inspect["proxySecret"],
        "fixture-proxy-secret-1234567890123456789"
    );
    run(
        &owner,
        "UPDATE webdock_auth.database_grant SET revision=revision+1 WHERE id=$1",
        vec![grant.clone().into()],
    )
    .await;
    assert!(service.inspect(token).await.is_err());
    let launch = service
        .execute(
            &actor,
            &authority,
            json!({"action":"open","bindingID":binding}),
        )
        .await
        .unwrap();
    let session = service
        .exchange(launch["code"].as_str().unwrap())
        .await
        .unwrap();
    let token = session["token"].as_str().unwrap();
    run(
        &owner,
        "DELETE FROM webdock_auth.member WHERE id=$1",
        vec![member.clone().into()],
    )
    .await;
    assert!(service.inspect(token).await.is_err());
    run(&owner,"INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES($1,$2,$3,'admin',now())",vec![member.clone().into(),org.clone().into(),user.clone().into()]).await;
    service.inspect(token).await.unwrap();
    service.logout(token).await.unwrap();
    assert!(service.inspect(token).await.is_err());
    let launch = service
        .execute(
            &actor,
            &authority,
            json!({"action":"open","bindingID":binding}),
        )
        .await
        .unwrap();
    run(
        &auth,
        "DELETE FROM webdock_auth.session WHERE id=$1",
        vec![actor.session_id.clone().into()],
    )
    .await;
    assert!(
        service
            .exchange(launch["code"].as_str().unwrap())
            .await
            .is_err()
    );
    for table in ["database_session", "database_launch"] {
        run(
            &owner,
            &format!("DELETE FROM webdock_auth.{table} WHERE grant_id=$1"),
            vec![grant.clone().into()],
        )
        .await;
    }
    for table in ["database_audit", "database_runtime_owner", "database_grant"] {
        run(
            &owner,
            &format!("DELETE FROM webdock_auth.{table} WHERE binding_id=$1"),
            vec![binding.clone().into()],
        )
        .await;
    }
    run(
        &owner,
        "DELETE FROM webdock_auth.database_binding WHERE id=$1",
        vec![binding.into()],
    )
    .await;
    run(
        &owner,
        "DELETE FROM webdock_admin.projects WHERE id=$1",
        vec![project.into()],
    )
    .await;
    run(
        &owner,
        "DELETE FROM webdock_auth.member WHERE id=$1",
        vec![member.into()],
    )
    .await;
    run(
        &owner,
        "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=$1",
        vec![customer.clone().into()],
    )
    .await;
    run(
        &owner,
        "DELETE FROM webdock_auth.organization WHERE id=$1",
        vec![org.into()],
    )
    .await;
    run(
        &owner,
        "DELETE FROM webdock_admin.customers WHERE id=$1",
        vec![customer.into()],
    )
    .await;
    run(
        &auth,
        "DELETE FROM webdock_auth.\"user\" WHERE id=$1",
        vec![user.into()],
    )
    .await;
    if module_added {
        run(
            &owner,
            "DELETE FROM webdock_auth.plugin_installation WHERE id='databases' AND version=$1",
            vec![module_version.into()],
        )
        .await;
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Explicit transaction, authority and revision inputs keep this operation boundary auditable."
)]
async fn managed_revisions(
    owner: &sea_orm::DatabaseConnection,
    service: &DatabaseService,
    actor: &spitzli_identity::Principal,
    authority: &StoredAuthority,
    ids: &spitzli_persistence::IdAllocator,
    cipher: &spitzli_secrets::SecretCipher,
    customer: &str,
    project: &str,
) {
    let cluster = ids.allocate().await.unwrap().to_string();
    let backend = ids.allocate().await.unwrap().to_string();
    let plan = ids.allocate().await.unwrap().to_string();
    let offer = ids.allocate().await.unwrap().to_string();
    let order = ids.allocate().await.unwrap().to_string();
    let database = ids.allocate().await.unwrap().to_string();
    run(owner,"INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence) VALUES($1,'Database fixture','docker','local','owned-fixture')",vec![cluster.clone().into()]).await;
    run(owner,"INSERT INTO webdock_auth.hosting_database_backend(id,cluster_id,kind,node,pool_id,demand,max_databases,logical_capacity_bytes,host,direct_port,pooled_port,state,endpoint_ready,ca_certificate) VALUES($1,$2,'dedicated','fixture','00000000-0000-4000-8000-000000000001','{}',1,1048576,'127.0.0.1',30001,30002,'ready',true,'fixture-only-ca')",vec![backend.clone().into(),cluster.clone().into()]).await;
    run(
        owner,
        "INSERT INTO webdock_auth.hosting_database_plan(id,plan) VALUES($1,'{}')",
        vec![plan.clone().into()],
    )
    .await;
    run(owner,"INSERT INTO webdock_auth.hosting_database_offer(id,customer_id,project_id,plan_revision_id,name,snapshot) VALUES($1,$2,$3,$4,'Database fixture','{}')",vec![offer.clone().into(),customer.into(),project.into(),plan.clone().into()]).await;
    run(owner,"INSERT INTO webdock_auth.hosting_database_order(id,offer_id,customer_id,project_id,plan_revision_id,accepted_by,snapshot) VALUES($1,$2,$3,$4,$5,$6,'{}')",vec![order.clone().into(),offer.clone().into(),customer.into(),project.into(),plan.clone().into(),actor.user_id.clone().into()]).await;
    let secret = cipher
        .seal(
            &json!({"migrationPassword":"managed-fixture-secret"}),
            &format!("database:{database}:1"),
        )
        .unwrap();
    run(owner,"INSERT INTO webdock_auth.hosting_database(id,customer_id,project_id,order_id,plan_revision_id,backend_id,kind,name,database_name,state,credentials_encrypted,confirmed_credentials_encrypted,confirmed_credential_revision) VALUES($1,$2,$3,$4,$5,$6,'dedicated','Database fixture',$7,'ready',$8,$8,1)",vec![database.clone().into(),customer.into(),project.into(),order.clone().into(),plan.clone().into(),backend.clone().into(),format!("fixture_{database}").into(),secret.into()]).await;
    let launch = service
        .execute(
            actor,
            authority,
            json!({"action":"openManaged","databaseID":database}),
        )
        .await
        .unwrap();
    let session = service
        .exchange(launch["code"].as_str().unwrap())
        .await
        .unwrap();
    let token = session["token"].as_str().unwrap();
    let access = service.inspect(token).await.unwrap();
    assert_eq!(access["kind"], "managed");
    assert_eq!(access["database"]["password"], "managed-fixture-secret");
    assert_eq!(
        access["database"]["username"],
        format!("wd_migrate_{database}")
    );
    run(
        owner,
        "UPDATE webdock_auth.hosting_database SET credential_revision=2 WHERE id=$1",
        vec![database.clone().into()],
    )
    .await;
    assert!(service.inspect(token).await.is_err());
    assert!(
        service
            .execute(
                actor,
                authority,
                json!({"action":"openManaged","databaseID":database})
            )
            .await
            .is_err()
    );
    run(owner,"UPDATE webdock_auth.hosting_database SET credential_revision=1,revision=revision+1 WHERE id=$1",vec![database.clone().into()]).await;
    assert!(service.inspect(token).await.is_err());
    let launch = service
        .execute(
            actor,
            authority,
            json!({"action":"openManaged","databaseID":database}),
        )
        .await
        .unwrap();
    run(
        owner,
        "UPDATE webdock_auth.hosting_database_backend SET endpoint_ready=false WHERE id=$1",
        vec![backend.clone().into()],
    )
    .await;
    assert!(
        service
            .exchange(launch["code"].as_str().unwrap())
            .await
            .is_err()
    );
    for table in ["managed_database_session", "managed_database_launch"] {
        run(
            owner,
            &format!("DELETE FROM webdock_auth.{table} WHERE database_id=$1"),
            vec![database.clone().into()],
        )
        .await;
    }
    run(
        owner,
        "DELETE FROM webdock_auth.hosting_audit WHERE target_id=$1 AND subject=$2",
        vec![database.clone().into(), actor.user_id.clone().into()],
    )
    .await;
    for (table, id) in [
        ("hosting_database", database),
        ("hosting_database_order", order),
        ("hosting_database_offer", offer),
        ("hosting_database_plan", plan),
        ("hosting_database_backend", backend),
        ("hosting_cluster", cluster),
    ] {
        run(
            owner,
            &format!("DELETE FROM webdock_auth.{table} WHERE id=$1"),
            vec![id.into()],
        )
        .await;
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Explicit transaction, authority and revision inputs keep this operation boundary auditable."
)]
async fn management_catalog(
    owner: &sea_orm::DatabaseConnection,
    service: &DatabaseService,
    hosting: &spitzli_hosting::HostingService,
    operator: &spitzli_identity::Principal,
    member: &spitzli_identity::Principal,
    authority: &StoredAuthority,
    customer: &str,
    project: &str,
    cluster: &str,
) {
    let plan = json!({"kind":"dedicated","postgresMajor":18,"cpuMillicores":500,"memoryBytes":1073741824,"volumeBytes":10737418240_u64,"logicalStorageAllowanceBytes":null,"name":"Private fixture plan","monthlyPriceCents":1200,"currency":"EUR","connectionLimit":10,"terms":"The customer accepts the fixed monthly database service terms for this isolated test."});
    let key = format!("plan-fixture-{project}");
    let command = json!({"action":"databases.plans.create","plan":plan,"idempotencyKey":key});
    assert!(
        service
            .execute_managed(member, authority, command.clone())
            .await
            .is_err()
    );
    let created = service
        .execute_managed(operator, authority, command.clone())
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();
    assert_eq!(created["availability"], "disabled");
    assert_eq!(
        service
            .execute_managed(operator, authority, command.clone())
            .await
            .unwrap(),
        created
    );
    let mut changed = command;
    changed["plan"]["name"] = json!("Different input");
    assert_eq!(
        service
            .execute_managed(operator, authority, changed)
            .await
            .unwrap_err()
            .status,
        409
    );
    let unavailable_offer = json!({"action":"databases.offers.create","projectID":project,"planRevisionID":id,"name":"Ordered fixture","idempotencyKey":format!("offer-fixture-{project}")});
    assert_eq!(
        service
            .execute_managed(operator, authority, unavailable_offer.clone())
            .await
            .unwrap_err()
            .status,
        409
    );
    let pilot=service.execute_managed(operator,authority,json!({"action":"databases.plans.setAvailability","planRevisionID":id,"availability":"pilot","revision":1,"idempotencyKey":format!("pilot-fixture-{project}")})).await.unwrap();
    assert_eq!(pilot["revision"], 2);
    assert!(
        service
            .execute_managed(member, authority, unavailable_offer.clone())
            .await
            .is_err()
    );
    let offer = service
        .execute_managed(operator, authority, unavailable_offer.clone())
        .await
        .unwrap();
    let capacity = json!({"apps":100,"cpuMillicores":16000,"memoryBytes":68719476736_u64,"volumeBytes":214748364800_u64,"ephemeralBytes":17179869184_u64,"replicasPerApp":10,"concurrentDeployments":10});
    let pool_id = "00000000-0000-4000-8000-000000000002";
    let observation = json!({"capacity":{"volumeBytes":107374182400_u64},"nodes":[{"id":"database-node","cpuMillicores":16000,"memoryBytes":68719476736_u64}],"capabilities":{"databaseVersion":1,"databaseNodePorts":[],"databasePools":[{"node":"database-node","poolID":pool_id,"ready":true,"capacityBytes":107374182400_u64,"availableBytes":107374182400_u64,"allocatedBytes":0,"requestedCpuMillicores":0,"requestedMemoryBytes":0,"allocations":[],"observedDatabases":[]}]}});
    run(owner,"INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence,verified,capacity) VALUES($1,'Database order fixture','k3s','local','owned-fixture',true,$2)",vec![cluster.into(),capacity.into()]).await;
    let credential = format!("{cluster:0>43}");
    use sha2::{Digest, Sha256};
    let credential_hash = hex::encode(Sha256::digest(credential.as_bytes()));
    run(owner,"INSERT INTO webdock_auth.hosting_agent(cluster_id,credential_hash,generation,last_seen,observation) VALUES($1,$2,1,now(),$3)",vec![cluster.into(),credential_hash.into(),observation.into()]).await;
    let pool_command = json!({"action":"databases.pools.register","clusterID":cluster,"node":"database-node","poolID":pool_id,"revision":1,"idempotencyKey":format!("pool-fixture-{project}")});
    service
        .execute_managed(operator, authority, pool_command)
        .await
        .unwrap();
    assert!(service.capacity_options(member, authority).await.is_err());
    let options = service.capacity_options(operator, authority).await.unwrap();
    let capacity = options
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == cluster)
        .unwrap();
    assert_eq!(capacity["ready"], true);
    assert!(
        capacity["pools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["poolID"] == pool_id && p["registered"] == true)
    );
    let accepted_command = json!({"action":"databases.acceptOrder","offerID":offer["id"],"projectID":project,"idempotencyKey":format!("accept-fixture-{project}")});
    let retired = retired_order_service(cluster).await;
    let mut new_quote = unavailable_offer.clone();
    new_quote["idempotencyKey"] = json!(format!("retired-offer-{project}"));
    for command in [new_quote, accepted_command.clone()] {
        let error = retired
            .execute_managed(operator, authority, command)
            .await
            .unwrap_err();
        assert_eq!(error.status, 409);
        assert!(error.message.contains("own Neon account"));
    }
    // A quote made before retirement can be read again, but cannot be newly accepted.
    assert_eq!(
        retired
            .execute_managed(operator, authority, unavailable_offer.clone())
            .await
            .unwrap(),
        offer
    );
    allocation_enqueue_failure_rolls_back(owner, operator, authority, &accepted_command, cluster)
        .await;
    let accepted = service
        .execute_managed(operator, authority, accepted_command.clone())
        .await
        .unwrap();
    assert_eq!(
        retired
            .execute_managed(operator, authority, accepted_command.clone())
            .await
            .unwrap(),
        accepted
    );
    let mut replay = accepted_command.clone();
    replay["idempotencyKey"] = json!(format!("retired-accepted-replay-{project}"));
    assert_eq!(
        retired
            .execute_managed(operator, authority, replay)
            .await
            .unwrap(),
        accepted
    );
    assert_eq!(
        service
            .execute_managed(operator, authority, accepted_command.clone())
            .await
            .unwrap(),
        accepted
    );
    saturated_order_and_agent_pools(operator, authority, &accepted_command, cluster).await;
    let database = accepted["databaseID"].as_str().unwrap();
    let order = accepted["orderID"].as_str().unwrap();
    let operation = accepted["operationID"].as_str().unwrap();
    let pending = retired
        .execute_managed(
            operator,
            authority,
            json!({"action":"databases.get","databaseID":database}),
        )
        .await
        .unwrap();
    assert_eq!(pending["state"], "provisioning");
    assert_eq!(pending["backup"]["status"], "not-configured");
    assert!(pending["endpoint"].is_null());
    assert_eq!(service.execute_managed(operator,authority,json!({"action":"databases.backup","databaseID":database,"revision":1,"idempotencyKey":format!("backup-fixture-{project}")})).await.unwrap_err().status,409);
    agent_lifecycle(
        owner,
        &retired,
        hosting,
        operator,
        authority,
        cluster,
        &credential,
        database,
        project,
        customer,
    )
    .await;
    upgrade_admission(
        owner, &retired, hosting, operator, authority, database, project, &plan,
    )
    .await;
    maintenance_admission(owner, &retired, operator, member, authority, database).await;
    let backend_row = owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT backend_id FROM webdock_auth.hosting_database WHERE id=$1",
            [database.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    let backend: String = backend_row.try_get("", "backend_id").unwrap();
    run(
        owner,
        "DELETE FROM webdock_auth.hosting_database_job WHERE operation_id=$1",
        vec![operation.into()],
    )
    .await;
    run(
        owner,
        "UPDATE webdock_auth.hosting_database SET operation_id=NULL WHERE id=$1",
        vec![database.into()],
    )
    .await;
    run(
        owner,
        "DELETE FROM webdock_auth.hosting_operation WHERE id=$1",
        vec![operation.into()],
    )
    .await;
    let cleanup = owner.begin().await.unwrap();
    let owned=cleanup.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT d.id FROM webdock_auth.hosting_database d JOIN webdock_auth.hosting_database_order o ON o.id=d.order_id AND o.database_id=d.id WHERE d.id=$1 AND o.id=$2 AND o.accepted_by=$3 AND d.project_id=$4 AND d.customer_id=$5 FOR UPDATE OF d,o",[database.into(),order.into(),operator.user_id.clone().into(),project.into(),customer.into()])).await.unwrap();
    assert!(owned.is_some());
    // Both exact synthetic rows form an intentional immutable FK cycle. Only this cleanup transaction bypasses its triggers.
    cleanup
        .execute_unprepared("SET LOCAL session_replication_role=replica")
        .await
        .unwrap();
    for (sql, id) in [
        (
            "DELETE FROM webdock_auth.hosting_database WHERE id=$1",
            database,
        ),
        (
            "DELETE FROM webdock_auth.hosting_database_order WHERE id=$1",
            order,
        ),
    ] {
        cleanup
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                [id.into()],
            ))
            .await
            .unwrap();
    }
    cleanup.commit().await.unwrap();
    run(
        owner,
        "DELETE FROM webdock_auth.hosting_database_port WHERE backend_id=$1",
        vec![backend.clone().into()],
    )
    .await;
    run(
        owner,
        "DELETE FROM webdock_auth.hosting_database_backend WHERE id=$1",
        vec![backend.into()],
    )
    .await;
    run(
        owner,
        "DELETE FROM webdock_auth.hosting_database_pool WHERE cluster_id=$1",
        vec![cluster.into()],
    )
    .await;
    for target in [database, cluster] {
        run(
            owner,
            "DELETE FROM webdock_auth.hosting_audit WHERE target_id=$1 AND subject=$2",
            vec![target.into(), operator.user_id.clone().into()],
        )
        .await;
    }
    for key in [
        format!("accept-fixture-{project}"),
        format!("retired-accepted-replay-{project}"),
        format!("pool-fixture-{project}"),
    ] {
        run(
            owner,
            "DELETE FROM webdock_auth.hosting_operation WHERE subject=$1 AND idempotency_key=$2",
            vec![operator.user_id.clone().into(), key.into()],
        )
        .await;
    }
    run(
        owner,
        "DELETE FROM webdock_auth.hosting_agent WHERE cluster_id=$1",
        vec![cluster.into()],
    )
    .await;
    run(
        owner,
        "DELETE FROM webdock_auth.hosting_cluster WHERE id=$1",
        vec![cluster.into()],
    )
    .await;
    assert_eq!(offer["snapshot"]["billingStarts"], "observed-ready");
    assert_eq!(offer["snapshot"]["backupStatus"], "not-configured");
    let listing = service
        .execute_managed(
            member,
            authority,
            json!({"action":"databases.projects.list","customerID":customer}),
        )
        .await
        .unwrap();
    assert!(
        listing
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == project)
    );
    let p = service
        .execute_managed(
            member,
            authority,
            json!({"action":"databases.permissions","projectID":project}),
        )
        .await
        .unwrap();
    assert_eq!(p["canWrite"], true);
    run(
        owner,
        "DELETE FROM webdock_auth.hosting_database_offer WHERE id=$1",
        vec![offer["id"].as_str().unwrap().into()],
    )
    .await;
    run(
        owner,
        "DELETE FROM webdock_auth.hosting_database_plan WHERE id=$1",
        vec![id.into()],
    )
    .await;
    run(
        owner,
        "DELETE FROM webdock_auth.hosting_audit WHERE target_id=$1 AND subject=$2",
        vec![id.into(), operator.user_id.clone().into()],
    )
    .await;
    for k in [
        key,
        format!("pilot-fixture-{project}"),
        format!("offer-fixture-{project}"),
    ] {
        run(
            owner,
            "DELETE FROM webdock_auth.hosting_operation WHERE subject=$1 AND idempotency_key=$2",
            vec![operator.user_id.clone().into(), k.into()],
        )
        .await;
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Explicit transaction, authority and revision inputs keep this operation boundary auditable."
)]
async fn agent_lifecycle(
    owner: &sea_orm::DatabaseConnection,
    service: &DatabaseService,
    hosting: &spitzli_hosting::HostingService,
    operator: &spitzli_identity::Principal,
    authority: &StoredAuthority,
    cluster: &str,
    credential: &str,
    database: &str,
    project: &str,
    customer: &str,
) {
    use spitzli_databases::*;
    let agent = hosting
        .authenticate_agent(cluster, credential)
        .await
        .unwrap();
    let claim = service
        .claim_database_operation(
            hosting.begin_authenticated_agent(&agent).await.unwrap(),
            &agent,
        )
        .await
        .unwrap()
        .unwrap();
    claim.validate(chrono::Utc::now()).unwrap();
    callback_lock_order(owner, service, hosting, &agent, &claim, None).await;
    assert_eq!(claim.spec.phase, DatabaseExecutionPhase::Apply);
    assert_eq!(claim.spec.database.as_ref().unwrap().id, database);
    assert!(claim.material.credentials.is_some());
    service
        .guard_database_operation(
            hosting.begin_authenticated_agent(&agent).await.unwrap(),
            &agent,
            &claim.lease,
        )
        .await
        .unwrap();
    let redelivered = service
        .claim_database_operation(
            hosting.begin_authenticated_agent(&agent).await.unwrap(),
            &agent,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(redelivered.spec.phase, DatabaseExecutionPhase::Apply);
    assert_eq!(
        redelivered.spec.fingerprint().unwrap(),
        claim.spec.fingerprint().unwrap(),
        "a lost Poll reply does not create a different command"
    );
    let tx = hosting.begin_authenticated_agent(&agent).await.unwrap();
    service
        .validate_database_callback_in(
            &tx,
            &agent,
            &claim.spec,
            &claim.lease,
            DatabaseCallback::Guard,
        )
        .await
        .unwrap();
    let mut forged = claim.spec.clone();
    forged.backend.resources.cpu_millicores += 1;
    assert!(
        service
            .validate_database_callback_in(
                &tx,
                &agent,
                &forged,
                &claim.lease,
                DatabaseCallback::Guard
            )
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    let mut wrong = claim.lease.clone();
    wrong.lease_token = "00".repeat(32);
    assert!(
        service
            .guard_database_operation(
                hosting.begin_authenticated_agent(&agent).await.unwrap(),
                &agent,
                &wrong
            )
            .await
            .is_err()
    );
    let mut params =
        rcgen::CertificateParams::new(vec!["database-fixture.invalid".into()]).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let key = rcgen::KeyPair::generate().unwrap();
    let ca = params.self_signed(&key).unwrap().pem();
    let checkpoint = DatabaseCheckpoint {
        lease: claim.lease.clone(),
        phase: DatabaseCheckpointPhase::Applied,
        cnpg_uid: "fixture-owned-cnpg".into(),
        ca_certificate: Some(ca.clone()),
        previous_pod_uid: None,
        migration_job_uid: None,
        copy_sha256: None,
    };
    callback_lock_order(owner, service, hosting, &agent, &claim, Some(&checkpoint)).await;
    let tx = hosting.begin_authenticated_agent(&agent).await.unwrap();
    service
        .validate_database_checkpoint_in(&tx, &agent, &claim.spec, &checkpoint)
        .await
        .unwrap();
    service
        .checkpoint_database_operation(tx, &agent, checkpoint.clone())
        .await
        .unwrap();
    let tx = hosting.begin_authenticated_agent(&agent).await.unwrap();
    service
        .validate_database_checkpoint_in(&tx, &agent, &claim.spec, &checkpoint)
        .await
        .unwrap();
    service
        .checkpoint_database_operation(tx, &agent, checkpoint.clone())
        .await
        .unwrap();
    run(owner,"UPDATE webdock_auth.hosting_operation SET lease_until=now()-interval '1 second' WHERE id=$1",vec![claim.spec.operation_id.clone().into()]).await;
    let tx = hosting.begin_authenticated_agent(&agent).await.unwrap();
    service
        .validate_database_checkpoint_in(&tx, &agent, &claim.spec, &checkpoint)
        .await
        .unwrap();
    service
        .checkpoint_database_operation(tx, &agent, checkpoint.clone())
        .await
        .unwrap();
    let expired: bool = owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT lease_until<=now() AS expired FROM webdock_auth.hosting_operation WHERE id=$1",
            [claim.spec.operation_id.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "expired")
        .unwrap();
    assert!(
        expired,
        "delivery-only checkpoint ACK cannot renew an expired lease"
    );
    assert!(
        service
            .guard_database_operation(
                hosting.begin_authenticated_agent(&agent).await.unwrap(),
                &agent,
                &claim.lease
            )
            .await
            .is_err()
    );
    let mut changed_expired = checkpoint.clone();
    changed_expired.previous_pod_uid = Some("different-checkpoint".into());
    assert!(
        service
            .checkpoint_database_operation(
                hosting.begin_authenticated_agent(&agent).await.unwrap(),
                &agent,
                changed_expired
            )
            .await
            .is_err()
    );
    // Restore only this synthetic clock fixture to exercise the independent remaining phases.
    run(owner,"UPDATE webdock_auth.hosting_operation SET lease_until=now()+interval '5 minutes' WHERE id=$1",vec![claim.spec.operation_id.clone().into()]).await;
    let mut changed = checkpoint.clone();
    changed.cnpg_uid = "foreign-cnpg".into();
    assert!(
        service
            .checkpoint_database_operation(
                hosting.begin_authenticated_agent(&agent).await.unwrap(),
                &agent,
                changed
            )
            .await
            .is_err()
    );
    let tx = hosting.begin_authenticated_agent(&agent).await.unwrap();
    assert!(
        service
            .validate_database_callback_in(
                &tx,
                &agent,
                &claim.spec,
                &claim.lease,
                DatabaseCallback::Guard
            )
            .await
            .is_err(),
        "an acknowledged apply cannot be reapplied"
    );
    tx.rollback().await.unwrap();
    let configuring = service
        .claim_database_operation(
            hosting.begin_authenticated_agent(&agent).await.unwrap(),
            &agent,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(configuring.spec.phase, DatabaseExecutionPhase::Configure);
    for phase in [
        DatabaseCheckpointPhase::Configuring,
        DatabaseCheckpointPhase::Observing,
    ] {
        let mut next = checkpoint.clone();
        next.phase = phase;
        next.ca_certificate = None;
        let tx = hosting.begin_authenticated_agent(&agent).await.unwrap();
        service
            .validate_database_checkpoint_in(&tx, &agent, &configuring.spec, &next)
            .await
            .unwrap();
        service
            .checkpoint_database_operation(tx, &agent, next)
            .await
            .unwrap();
        if phase == DatabaseCheckpointPhase::Configuring {
            let uncertain = service
                .claim_database_operation(
                    hosting.begin_authenticated_agent(&agent).await.unwrap(),
                    &agent,
                )
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                uncertain.spec.phase,
                DatabaseExecutionPhase::Uncertain,
                "interrupted configuration never replays mutations"
            );
            let tx = hosting.begin_authenticated_agent(&agent).await.unwrap();
            service
                .validate_database_callback_in(
                    &tx,
                    &agent,
                    &configuring.spec,
                    &configuring.lease,
                    DatabaseCallback::Guard,
                )
                .await
                .unwrap();
            tx.rollback().await.unwrap();
        }
    }
    let observed = service
        .claim_database_operation(
            hosting.begin_authenticated_agent(&agent).await.unwrap(),
            &agent,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observed.spec.phase, DatabaseExecutionPhase::Observe);
    let proof = DatabaseCompletionProof {
        installation_id: claim.spec.backend.id.clone(),
        database_id: Some(database.into()),
        cnpg_uid: "fixture-owned-cnpg".into(),
        revision: 1,
        state: DatabaseProofState::Ready,
        endpoint_ready: true,
        copy_sha256: None,
        backup: None,
    };
    let completion = DatabaseCompletion {
        lease: claim.lease.clone(),
        outcome: DatabaseCompletionOutcome::Succeeded,
        proof: Some(proof),
    };
    let tx = hosting.begin_authenticated_agent(&agent).await.unwrap();
    service
        .validate_database_callback_in(
            &tx,
            &agent,
            &observed.spec,
            &observed.lease,
            DatabaseCallback::Completion(completion.outcome),
        )
        .await
        .unwrap();
    service
        .complete_database_operation(tx, &agent, completion.clone())
        .await
        .unwrap();
    run(owner,"UPDATE webdock_auth.hosting_operation SET lease_until=now()-interval '1 minute' WHERE id=$1",vec![claim.spec.operation_id.clone().into()]).await;
    let tx = hosting.begin_authenticated_agent(&agent).await.unwrap();
    service
        .validate_database_callback_in(
            &tx,
            &agent,
            &observed.spec,
            &observed.lease,
            DatabaseCallback::Completion(completion.outcome),
        )
        .await
        .unwrap();
    service
        .complete_database_operation(tx, &agent, completion.clone())
        .await
        .unwrap();
    let mut bad = completion;
    bad.proof.as_mut().unwrap().endpoint_ready = false;
    assert!(
        service
            .complete_database_operation(
                hosting.begin_authenticated_agent(&agent).await.unwrap(),
                &agent,
                bad
            )
            .await
            .is_err()
    );
    let ready = service
        .execute_managed(
            operator,
            authority,
            json!({"action":"databases.get","databaseID":database}),
        )
        .await
        .unwrap();
    assert_eq!(ready["state"], "ready");
    let observed = service
        .database_observation_config(
            hosting.begin_authenticated_agent(&agent).await.unwrap(),
            &agent,
        )
        .await
        .unwrap();
    assert!(
        observed["installations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["id"] == claim.spec.backend.id)
    );
    let mut report = json!({"observedAt":chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis,true),"installations":[{"installationID":claim.spec.backend.id,"cnpgUID":"foreign-cnpg","ready":true,"filesystemBytes":1024,"databases":[{"id":database,"logicalBytes":42}]}]});
    assert!(
        service
            .report_database_observations(
                hosting.begin_authenticated_agent(&agent).await.unwrap(),
                &agent,
                report.clone()
            )
            .await
            .is_err()
    );
    report["installations"][0]["cnpgUID"] = json!("fixture-owned-cnpg");
    service
        .report_database_observations(
            hosting.begin_authenticated_agent(&agent).await.unwrap(),
            &agent,
            report,
        )
        .await
        .unwrap();
    let credentials = service
        .display_credentials(operator, authority, database)
        .await
        .unwrap();
    assert_eq!(
        credentials["application"]["password"],
        claim
            .material
            .credentials
            .as_ref()
            .unwrap()
            .application_password
    );
    let tx = hosting.begin_authenticated_agent(&agent).await.unwrap();
    let request = DatabaseBindingRequest {
        database_id: database.into(),
        project_id: project.into(),
        customer_id: customer.into(),
        cluster_id: cluster.into(),
        binding_id: "123".into(),
        variable: "DATABASE_URL".into(),
    };
    let binding = service.binding_material_in(&tx, &request).await.unwrap();
    assert_eq!(binding.credential_revision, 1);
    assert_eq!(binding.runtime.ca_certificate, ca);
    assert!(binding.environment["DATABASE_URL"].contains("sslmode=verify-full"));
    let scope: serde_json::Value = tx
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT webdock_rls.request_scope() AS scope",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "scope")
        .unwrap();
    assert_eq!(
        scope["purpose"], "agent",
        "module metadata scope is restored"
    );
    tx.rollback().await.unwrap();
    if std::env::var("WEBDOCK_BACKUP_AUTHORITY_TEST").as_deref() == Ok("yes") {
        backup_policy_authority_fixture(
            owner, service, hosting, operator, authority, &agent, database,
        )
        .await;
    }
}

async fn backup_policy_authority_fixture(
    owner: &sea_orm::DatabaseConnection,
    service: &DatabaseService,
    hosting: &spitzli_hosting::HostingService,
    operator: &spitzli_identity::Principal,
    authority: &StoredAuthority,
    agent: &spitzli_hosting::AgentIdentity,
    database: &str,
) {
    use spitzli_databases::*;
    let original_observation = fixture_value(
        owner,
        "SELECT observation AS value FROM webdock_auth.hosting_agent WHERE cluster_id=$1",
        agent.cluster_id(),
    )
    .await;
    run(owner,"UPDATE webdock_auth.hosting_agent SET observation=jsonb_set(observation,'{capabilities,databaseBackupVersion}','1') WHERE cluster_id=$1",vec![agent.cluster_id().into()]).await;

    let before=fixture_value(owner,"SELECT demand AS value FROM webdock_auth.hosting_database_backend WHERE id=(SELECT backend_id FROM webdock_auth.hosting_database WHERE id=$1)",database).await;
    let cmd = json!({"action":"databases.backupDestinations.register","endpointURL":"https://backup.fixture.invalid","region":"eu-central-1","bucket":"backup-fixture","keyRecoveryID":"110","writeCredentials":{"accessKeyId":"write-fixture","secretAccessKey":"local-fixture-write-key-material","readOnly":false},"recoveryCredentials":{"accessKeyId":"read-fixture","secretAccessKey":"local-fixture-read-key-material","readOnly":true},"idempotencyKey":format!("backup-destination-{database}")});
    let destination = service
        .execute_managed(operator, authority, cmd)
        .await
        .unwrap();
    let grant_command = json!({"action":"databases.backupPolicies.grant","databaseID":database,"revision":1,"destinationID":destination["destinationID"],"schedule":"0 0 1 * * *","retentionDays":30,"retentionDelete":false,"idempotencyKey":format!("backup-grant-{database}")});
    let capacity = fixture_value(
        owner,
        "SELECT capacity AS value FROM webdock_auth.hosting_cluster WHERE id=$1",
        agent.cluster_id(),
    )
    .await;
    run(owner,"UPDATE webdock_auth.hosting_cluster SET capacity=jsonb_set(capacity,'{ephemeralBytes}','0') WHERE id=$1",vec![agent.cluster_id().into()]).await;
    assert_eq!(
        service
            .execute_managed(operator, authority, grant_command.clone())
            .await
            .unwrap_err()
            .status,
        409,
        "backup and recovery ephemeral peak must fit"
    );
    run(
        owner,
        "UPDATE webdock_auth.hosting_cluster SET capacity=$2 WHERE id=$1",
        vec![agent.cluster_id().into(), capacity.into()],
    )
    .await;
    let granted = service
        .execute_managed(operator, authority, grant_command)
        .await
        .unwrap();
    let apply = service
        .claim_database_operation(
            hosting.begin_authenticated_agent(agent).await.unwrap(),
            agent,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(apply.spec.action, DatabaseAction::ConfigureBackup);
    assert!(
        !serde_json::to_string(&apply.spec)
            .unwrap()
            .contains("key-material")
    );
    assert!(apply.material.object_store.is_some());
    assert_eq!(
        apply.spec.backup.as_ref().unwrap().sidecar.memory_bytes,
        268435456,
        "native sidecars reserve the measured local minimum, rather than the OOM-killed 128 MiB profile"
    );
    run(owner,"UPDATE webdock_auth.hosting_operation SET lease_until=now()+interval '45 seconds' WHERE id=$1",vec![apply.spec.operation_id.clone().into()]).await;
    let renewed = service
        .claim_database_operation(
            hosting.begin_authenticated_agent(agent).await.unwrap(),
            agent,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(
        renewed.lease_until > chrono::Utc::now() + chrono::Duration::seconds(240),
        "an authenticated unchanged claim renews long pending recovery work"
    );
    assert_eq!(
        renewed.spec.fingerprint().unwrap(),
        apply.spec.fingerprint().unwrap()
    );
    assert_eq!(renewed.lease.lease_token, apply.lease.lease_token);
    let expiry = fixture_value(
        owner,
        "SELECT to_jsonb(\"expiresAt\") AS value FROM webdock_auth.session WHERE id=$1",
        &operator.session_id,
    )
    .await;
    run(
        owner,
        "UPDATE webdock_auth.session SET \"expiresAt\"=now()-interval '1 minute' WHERE id=$1",
        vec![operator.session_id.clone().into()],
    )
    .await;
    service
        .guard_database_operation(
            hosting.begin_authenticated_agent(agent).await.unwrap(),
            agent,
            &apply.lease,
        )
        .await
        .unwrap();
    assert!(
        service
            .execute_managed(
                operator,
                authority,
                json!({"action":"databases.backupPolicies.get","databaseID":database})
            )
            .await
            .is_err(),
        "session is really expired"
    );
    run(
        owner,
        "UPDATE webdock_auth.session SET \"expiresAt\"=$2::text::timestamptz WHERE id=$1",
        vec![
            operator.session_id.clone().into(),
            expiry.as_str().unwrap().into(),
        ],
    )
    .await;
    let revoked=service.execute_managed(operator,authority,json!({"action":"databases.backupPolicies.revoke","databaseID":database,"revision":1,"policyRevision":1,"idempotencyKey":format!("backup-revoke-{database}")})).await.unwrap();
    assert_eq!(revoked["revocationEffective"], false);
    assert!(
        service
            .guard_database_operation(
                hosting.begin_authenticated_agent(agent).await.unwrap(),
                agent,
                &apply.lease
            )
            .await
            .is_err()
    );
    let mut job = service
        .claim_database_operation(
            hosting.begin_authenticated_agent(agent).await.unwrap(),
            agent,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.spec.backup.as_ref().unwrap().revision, 2);
    assert!(job.spec.backup.as_ref().unwrap().suspended);
    for phase in [
        DatabaseCheckpointPhase::Applied,
        DatabaseCheckpointPhase::Configuring,
        DatabaseCheckpointPhase::Observing,
    ] {
        service
            .checkpoint_database_operation(
                hosting.begin_authenticated_agent(agent).await.unwrap(),
                agent,
                DatabaseCheckpoint {
                    lease: job.lease.clone(),
                    phase,
                    cnpg_uid: job.spec.backend.cnpg_uid.clone().unwrap(),
                    ca_certificate: None,
                    previous_pod_uid: None,
                    migration_job_uid: None,
                    copy_sha256: None,
                },
            )
            .await
            .unwrap();
        if phase == DatabaseCheckpointPhase::Applied {
            job = service
                .claim_database_operation(
                    hosting.begin_authenticated_agent(agent).await.unwrap(),
                    agent,
                )
                .await
                .unwrap()
                .unwrap();
        }
    }
    let observed = service
        .claim_database_operation(
            hosting.begin_authenticated_agent(agent).await.unwrap(),
            agent,
        )
        .await
        .unwrap()
        .unwrap();
    let proof = DatabaseCompletionProof {
        installation_id: observed.spec.backend.id.clone(),
        database_id: Some(database.into()),
        cnpg_uid: observed.spec.backend.cnpg_uid.clone().unwrap(),
        revision: 1,
        state: DatabaseProofState::Ready,
        endpoint_ready: false,
        copy_sha256: None,
        backup: Some(Box::new(DatabaseBackupProof {
            policy_id: granted["policyID"].as_str().unwrap().into(),
            revision: 2,
            scheduled_backup_uid: "fixture-schedule-uid".into(),
            controls: DatabaseBackupControls {
                schedule: "0 0 1 * * *".into(),
                wal_enabled: true,
                retention_delete: false,
                retention_days: 30,
                scheduled_backup_resource_version: "1".into(),
                cluster_resource_version: "2".into(),
                object_store_uid: "fixture-store-uid".into(),
                object_store_resource_version: "3".into(),
            },
            suspended: true,
            backup_uid: None,
            base_backup_id: None,
            completed_at: None,
        })),
    };
    service
        .complete_database_operation(
            hosting.begin_authenticated_agent(agent).await.unwrap(),
            agent,
            DatabaseCompletion {
                lease: observed.lease,
                outcome: DatabaseCompletionOutcome::Succeeded,
                proof: Some(proof),
            },
        )
        .await
        .unwrap();
    let status = service
        .execute_managed(
            operator,
            authority,
            json!({"action":"databases.backupPolicies.get","databaseID":database}),
        )
        .await
        .unwrap();
    assert_eq!(status["policy"]["revocationEffective"], true);
    let managed = service
        .execute_managed(
            operator,
            authority,
            json!({"action":"databases.get","databaseID":database}),
        )
        .await
        .unwrap();
    let schema: serde_json::Value =
        serde_json::from_str(spitzli_databases::BACKUP_STATUS_SCHEMA).unwrap();
    assert!(
        jsonschema::validator_for(&schema)
            .unwrap()
            .is_valid(&managed["backup"])
    );
    assert_eq!(managed["backup"]["publicRecoveryAvailable"], false);
    // Exact synthetic records only, following the existing fixture's FK-cycle cleanup.
    let cleanup = owner.begin().await.unwrap();
    cleanup
        .execute_unprepared("SET LOCAL session_replication_role=replica")
        .await
        .unwrap();
    for sql in [
        "DELETE FROM webdock_auth.hosting_database_backup_audit WHERE policy_id=$1",
        "DELETE FROM webdock_auth.hosting_database_backup_credentials WHERE policy_id=$1",
        "DELETE FROM webdock_auth.hosting_database_backup WHERE policy_id=$1",
        "DELETE FROM webdock_auth.hosting_database_job WHERE backup_policy_id=$1",
        "DELETE FROM webdock_auth.hosting_database_backup_policy WHERE id=$1",
    ] {
        cleanup
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                [granted["policyID"].as_str().unwrap().into()],
            ))
            .await
            .unwrap();
    }
    cleanup
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.hosting_database_backup_destination WHERE id=$1",
            [destination["destinationID"].as_str().unwrap().into()],
        ))
        .await
        .unwrap();
    for op in [&granted["operationID"], &revoked["operationID"]] {
        cleanup
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM webdock_auth.hosting_operation WHERE id=$1",
                [op.as_str().unwrap().into()],
            ))
            .await
            .unwrap();
    }
    for prefix in ["backup-destination", "backup-grant", "backup-revoke"] {
        cleanup.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"DELETE FROM webdock_auth.hosting_operation WHERE subject=$1 AND idempotency_key=$2",[operator.user_id.clone().into(),format!("{prefix}-{database}").into()])).await.unwrap();
    }
    cleanup.commit().await.unwrap();
    run(
        owner,
        "UPDATE webdock_auth.hosting_database_backend SET demand=$2 WHERE id=$1",
        vec![apply.spec.backend.id.into(), before.into()],
    )
    .await;
    run(
        owner,
        "UPDATE webdock_auth.hosting_agent SET observation=$2 WHERE cluster_id=$1",
        vec![agent.cluster_id().into(), original_observation.into()],
    )
    .await;
}

async fn fixture_value(
    owner: &sea_orm::DatabaseConnection,
    sql: &str,
    id: &str,
) -> serde_json::Value {
    owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            [id.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "value")
        .unwrap()
}
#[expect(
    clippy::too_many_arguments,
    reason = "Explicit transaction, authority and revision inputs keep this operation boundary auditable."
)]
async fn upgrade_admission(
    owner: &sea_orm::DatabaseConnection,
    service: &DatabaseService,
    hosting: &spitzli_hosting::HostingService,
    actor: &spitzli_identity::Principal,
    authority: &StoredAuthority,
    database: &str,
    project: &str,
    original_plan: &serde_json::Value,
) {
    let original = fixture_value(
        owner,
        "SELECT to_jsonb(d) AS value FROM webdock_auth.hosting_database d WHERE id=$1",
        database,
    )
    .await;
    let original_order = fixture_value(
        owner,
        "SELECT to_jsonb(o) AS value FROM webdock_auth.hosting_database_order o WHERE id=$1",
        original["order_id"].as_str().unwrap(),
    )
    .await;
    let backend_id = original["backend_id"].as_str().unwrap();
    let backend = fixture_value(
        owner,
        "SELECT to_jsonb(b) AS value FROM webdock_auth.hosting_database_backend b WHERE id=$1",
        backend_id,
    )
    .await;
    for grow_volume in [false, true] {
        let prefix = format!("upgrade-{database}-{grow_volume}");
        let mut plan = original_plan.clone();
        plan["cpuMillicores"] = json!(1000);
        if grow_volume {
            plan["volumeBytes"] = json!(21474836480_u64);
        }
        let made=service.execute_managed(actor,authority,json!({"action":"databases.plans.create","plan":plan,"idempotencyKey":format!("{prefix}-plan")})).await.unwrap();
        service.execute_managed(actor,authority,json!({"action":"databases.plans.setAvailability","planRevisionID":made["id"],"availability":"pilot","revision":1,"idempotencyKey":format!("{prefix}-pilot")})).await.unwrap();
        let offer=service.execute_managed(actor,authority,json!({"action":"databases.offers.create","projectID":project,"databaseID":database,"planRevisionID":made["id"],"name":"Upgrade fixture","idempotencyKey":format!("{prefix}-offer")})).await.unwrap();
        let mut command = json!({"action":"databases.acceptOrder","projectID":project,"offerID":offer["id"],"idempotencyKey":format!("{prefix}-accept")});
        assert_eq!(
            service
                .execute_managed(actor, authority, command.clone())
                .await
                .unwrap_err()
                .status,
            409
        );
        command["confirmInterruption"] = json!(true);
        let result = service
            .execute_managed(actor, authority, command.clone())
            .await
            .expect("confirmed upgrade creates a fenced operation");
        assert_eq!(
            service
                .execute_managed(actor, authority, command)
                .await
                .unwrap(),
            result
        );
        let job=fixture_value(owner,"SELECT to_jsonb(j) AS value FROM webdock_auth.hosting_database_job j WHERE operation_id=$1",result["operationID"].as_str().unwrap()).await;
        assert_eq!(
            job["action"],
            if grow_volume {
                "prepareMigration"
            } else {
                "resize"
            }
        );
        assert_eq!(job["backend_id"] == backend_id, !grow_volume);
        let still = fixture_value(
            owner,
            "SELECT to_jsonb(b) AS value FROM webdock_auth.hosting_database_backend b WHERE id=$1",
            backend_id,
        )
        .await;
        assert_eq!(
            still["profile"]["volumeBytes"],
            backend["profile"]["volumeBytes"]
        );
        let changed = fixture_value(
            owner,
            "SELECT to_jsonb(d) AS value FROM webdock_auth.hosting_database d WHERE id=$1",
            database,
        )
        .await;
        assert_eq!(changed["pending_order_id"], result["orderID"]);
        assert_eq!(changed["state"], "updating");
        if grow_volume {
            assert_ne!(changed["pending_backend_id"], original["backend_id"]);
            assert_eq!(changed["credential_revision"], 2);
        }
        let cluster = backend["cluster_id"].as_str().unwrap();
        let agent = hosting
            .authenticate_agent(cluster, &format!("{cluster:0>43}"))
            .await
            .unwrap();
        if grow_volume {
            let (prepare, response) = complete_synthetic_stage(
                service,
                hosting,
                &agent,
                spitzli_databases::DatabaseAction::PrepareMigration,
            )
            .await;
            assert_eq!(response["pending"], true);
            let (migrate, response) = complete_synthetic_stage(
                service,
                hosting,
                &agent,
                spitzli_databases::DatabaseAction::Migrate,
            )
            .await;
            assert_eq!(response["accepted"], true);
            assert_ne!(prepare.lease.lease_token, migrate.lease.lease_token);
            assert_eq!(
                service
                    .complete_database_operation(
                        hosting.begin_authenticated_agent(&agent).await.unwrap(),
                        &agent,
                        prepare
                    )
                    .await
                    .unwrap()["pending"],
                true
            );
            let cutover = fixture_value(
                owner,
                "SELECT to_jsonb(d) AS value FROM webdock_auth.hosting_database d WHERE id=$1",
                database,
            )
            .await;
            assert_eq!(cutover["backend_id"], job["backend_id"]);
            assert_eq!(cutover["order_id"], result["orderID"]);
            assert_eq!(cutover["confirmed_credential_revision"], 2);
            assert_eq!(cutover["state"], "ready");
            let source=fixture_value(owner,"SELECT to_jsonb(b) AS value FROM webdock_auth.hosting_database_backend b WHERE id=$1",backend_id).await;
            assert_eq!(source["state"], "retained");
            let migrations = service
                .execute_managed(
                    actor,
                    authority,
                    json!({"action":"databases.migrations.list","databaseID":database}),
                )
                .await
                .unwrap();
            let migration_id = migrations[0]["id"].as_str().unwrap();
            assert_eq!(migrations[0]["state"], "retained");
            let preview = service
                .execute_managed(
                    actor,
                    authority,
                    json!({"action":"databases.migrationPurgePreview","migrationID":migration_id}),
                )
                .await
                .unwrap();
            let purge = json!({"action":"databases.purgeMigrationSource","migrationID":migration_id,"revision":cutover["revision"],"confirmName":cutover["name"],"planHash":preview["planHash"],"idempotencyKey":format!("{prefix}-purge-source")});
            assert_eq!(
                service
                    .execute_managed(actor, authority, purge.clone())
                    .await
                    .unwrap_err()
                    .status,
                409
            );
            run(owner,"UPDATE webdock_auth.hosting_database_migration SET retained_until=now()-interval '1 second' WHERE id=$1 AND database_id=$2",vec![migration_id.into(),database.into()]).await;
            let purged = service
                .execute_managed(actor, authority, purge)
                .await
                .unwrap();
            complete_synthetic_stage(
                service,
                hosting,
                &agent,
                spitzli_databases::DatabaseAction::Purge,
            )
            .await;
            let erased=fixture_value(owner,"SELECT to_jsonb(m) AS value FROM webdock_auth.hosting_database_migration m WHERE id=$1",migration_id).await;
            assert_eq!(erased["state"], "purged");
            assert_eq!(erased["source_credentials_encrypted"], "");
            assert_eq!(erased["transfer_credentials_encrypted"], "");
            for sql in [
                "DELETE FROM webdock_auth.hosting_database_job WHERE operation_id=$1",
                "DELETE FROM webdock_auth.hosting_operation WHERE id=$1",
            ] {
                run(
                    owner,
                    sql,
                    vec![purged["operationID"].as_str().unwrap().into()],
                )
                .await;
            }
            run(
                owner,
                "DELETE FROM webdock_auth.hosting_audit WHERE target_id=$1 AND subject=$2",
                vec![migration_id.into(), actor.user_id.clone().into()],
            )
            .await;
        } else {
            complete_synthetic_stage(
                service,
                hosting,
                &agent,
                spitzli_databases::DatabaseAction::Resize,
            )
            .await;
        }
        let wrapper = json!({"action":"databases.resize","databaseID":database,"revision":original["revision"],"targetOrderID":result["orderID"],"targetPlanRevisionID":made["id"],"idempotencyKey":format!("{prefix}-wrapper")});
        let previous_view = service
            .execute_managed(actor, authority, wrapper.clone())
            .await
            .unwrap();
        run(owner,"UPDATE webdock_auth.hosting_database SET pending_order_id=NULL,pending_plan_revision_id=NULL WHERE id=$1",vec![database.into()]).await;
        assert_eq!(
            service
                .execute_managed(actor, authority, wrapper)
                .await
                .expect("committed wrapper replay survives a later database revision"),
            previous_view
        );
        // Restore only this test's exact synthetic rows; the original cyclic order/database pair is immutable.
        let tx = owner.begin().await.unwrap();
        tx.execute_unprepared("SET LOCAL session_replication_role=replica")
            .await
            .unwrap();
        let statements = vec![
            (
                "DELETE FROM webdock_auth.hosting_database_migration WHERE database_id=$1 AND target_order_id=$2",
                vec![database.into(), result["orderID"].as_str().unwrap().into()],
            ),
            (
                "DELETE FROM webdock_auth.hosting_database_job WHERE operation_id=$1",
                vec![result["operationID"].as_str().unwrap().into()],
            ),
            (
                "DELETE FROM webdock_auth.hosting_operation WHERE id=$1",
                vec![result["operationID"].as_str().unwrap().into()],
            ),
            (
                "DELETE FROM webdock_auth.hosting_database_order WHERE id=$1 AND database_id=$2",
                vec![result["orderID"].as_str().unwrap().into(), database.into()],
            ),
            (
                "DELETE FROM webdock_auth.hosting_database WHERE id=$1",
                vec![database.into()],
            ),
            (
                "INSERT INTO webdock_auth.hosting_database SELECT * FROM jsonb_populate_record(NULL::webdock_auth.hosting_database,$1)",
                vec![original.clone().into()],
            ),
            (
                "DELETE FROM webdock_auth.hosting_database_backend WHERE id=$1",
                vec![backend_id.into()],
            ),
            (
                "INSERT INTO webdock_auth.hosting_database_backend SELECT * FROM jsonb_populate_record(NULL::webdock_auth.hosting_database_backend,$1)",
                vec![backend.clone().into()],
            ),
        ];
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.hosting_database_order SET ended_at=(jsonb_populate_record(NULL::webdock_auth.hosting_database_order,$2)).ended_at WHERE id=$1",[original["order_id"].as_str().unwrap().into(),original_order.clone().into()])).await.unwrap();
        for (sql, values) in statements {
            tx.execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                values,
            ))
            .await
            .unwrap();
        }
        if grow_volume {
            let target = job["backend_id"].as_str().unwrap();
            for sql in [
                "DELETE FROM webdock_auth.hosting_database_port WHERE backend_id=$1",
                "DELETE FROM webdock_auth.hosting_database_backend WHERE id=$1",
            ] {
                tx.execute_raw(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    sql,
                    [target.into()],
                ))
                .await
                .unwrap();
            }
        }
        tx.commit().await.unwrap();
        run(
            owner,
            "DELETE FROM webdock_auth.hosting_database_offer WHERE id=$1",
            vec![offer["id"].as_str().unwrap().into()],
        )
        .await;
        run(
            owner,
            "DELETE FROM webdock_auth.hosting_database_plan WHERE id=$1",
            vec![made["id"].as_str().unwrap().into()],
        )
        .await;
        run(
            owner,
            "DELETE FROM webdock_auth.hosting_audit WHERE target_id=$1 AND subject=$2",
            vec![
                made["id"].as_str().unwrap().into(),
                actor.user_id.clone().into(),
            ],
        )
        .await;
        for suffix in [
            "plan",
            "pilot",
            "offer",
            "accept",
            "wrapper",
            "purge-source",
        ] {
            run(owner,"DELETE FROM webdock_auth.hosting_operation WHERE subject=$1 AND idempotency_key=$2",vec![actor.user_id.clone().into(),format!("{prefix}-{suffix}").into()]).await;
        }
    }
}

async fn maintenance_admission(
    owner: &sea_orm::DatabaseConnection,
    service: &DatabaseService,
    actor: &spitzli_identity::Principal,
    member: &spitzli_identity::Principal,
    authority: &StoredAuthority,
    database: &str,
) {
    let original = fixture_value(
        owner,
        "SELECT to_jsonb(d) AS value FROM webdock_auth.hosting_database d WHERE id=$1",
        database,
    )
    .await;
    assert!(
        service
            .execute_managed(
                member,
                authority,
                json!({"action":"databases.deletionPreview","databaseID":database})
            )
            .await
            .is_err()
    );
    for action in ["pause", "restart", "rotateCredentials", "delete", "purge"] {
        let key = format!("maintenance-{database}-{action}");
        let mut command = json!({"action":format!("databases.{action}"),"databaseID":database,"revision":original["revision"],"idempotencyKey":key});
        if ["delete", "purge"].contains(&action) {
            let preview=service.execute_managed(actor,authority,json!({"action":if action=="delete"{"databases.deletionPreview"}else{"databases.purgePreview"},"databaseID":database})).await.unwrap();
            command["planHash"] = preview["planHash"].clone();
            command["confirmName"] = json!("Wrong database");
            assert_eq!(
                service
                    .execute_managed(actor, authority, command.clone())
                    .await
                    .unwrap_err()
                    .status,
                409
            );
            command["confirmName"] = original["name"].clone();
            if action == "purge" {
                assert_eq!(
                    service
                        .execute_managed(actor, authority, command.clone())
                        .await
                        .unwrap_err()
                        .status,
                    409
                );
                run(owner,"UPDATE webdock_auth.hosting_database SET state='retained',retained_until=now()-interval '1 second' WHERE id=$1",vec![database.into()]).await;
            }
        }
        let result = service
            .execute_managed(actor, authority, command.clone())
            .await
            .unwrap();
        assert_eq!(
            service
                .execute_managed(actor, authority, command)
                .await
                .unwrap(),
            result
        );
        let changed = fixture_value(
            owner,
            "SELECT to_jsonb(d) AS value FROM webdock_auth.hosting_database d WHERE id=$1",
            database,
        )
        .await;
        assert_eq!(
            changed["revision"].as_i64().unwrap(),
            original["revision"].as_i64().unwrap() + 1
        );
        if action == "rotateCredentials" {
            assert_ne!(
                changed["credentials_encrypted"],
                original["credentials_encrypted"]
            );
            assert!(
                service
                    .display_credentials(actor, authority, database)
                    .await
                    .is_err()
            );
        }
        let operation = changed["operation_id"].as_str().unwrap();
        let job=fixture_value(owner,"SELECT to_jsonb(j) AS value FROM webdock_auth.hosting_database_job j WHERE operation_id=$1",operation).await;
        assert_eq!(job["action"], action);
        assert_eq!(service.execute_managed(actor,authority,json!({"action":"databases.restart","databaseID":database,"revision":changed["revision"],"idempotencyKey":format!("{key}-busy")})).await.unwrap_err().status,409);
        let tx = owner.begin().await.unwrap();
        tx.execute_unprepared("SET LOCAL session_replication_role=replica")
            .await
            .unwrap();
        for (sql, values) in [
            (
                "DELETE FROM webdock_auth.hosting_database_job WHERE operation_id=$1",
                vec![operation.into()],
            ),
            (
                "DELETE FROM webdock_auth.hosting_operation WHERE id=$1",
                vec![operation.into()],
            ),
            (
                "DELETE FROM webdock_auth.hosting_database WHERE id=$1",
                vec![database.into()],
            ),
            (
                "INSERT INTO webdock_auth.hosting_database SELECT * FROM jsonb_populate_record(NULL::webdock_auth.hosting_database,$1)",
                vec![original.clone().into()],
            ),
        ] {
            tx.execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                values,
            ))
            .await
            .unwrap();
        }
        tx.commit().await.unwrap();
        run(
            owner,
            "DELETE FROM webdock_auth.hosting_operation WHERE subject=$1 AND idempotency_key=$2",
            vec![actor.user_id.clone().into(), key.into()],
        )
        .await;
    }
    run(
        owner,
        "DELETE FROM webdock_auth.hosting_database_preview WHERE database_id=$1",
        vec![database.into()],
    )
    .await;
}

async fn complete_synthetic_stage(
    service: &DatabaseService,
    hosting: &spitzli_hosting::HostingService,
    agent: &spitzli_hosting::AgentIdentity,
    action: spitzli_databases::DatabaseAction,
) -> (spitzli_databases::DatabaseCompletion, serde_json::Value) {
    use spitzli_databases::*;
    let claim = service
        .claim_database_operation(
            hosting.begin_authenticated_agent(agent).await.unwrap(),
            agent,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.spec.action, action);
    assert_eq!(claim.spec.phase, DatabaseExecutionPhase::Apply);
    let uid = claim
        .spec
        .backend
        .cnpg_uid
        .clone()
        .unwrap_or_else(|| format!("synthetic-{}", claim.spec.backend.id));
    let ca = if claim.spec.backend.cnpg_uid.is_none() {
        let mut p =
            rcgen::CertificateParams::new(vec!["migration-fixture.invalid".into()]).unwrap();
        p.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        Some(
            p.self_signed(&rcgen::KeyPair::generate().unwrap())
                .unwrap()
                .pem(),
        )
    } else {
        None
    };
    let mut cp = DatabaseCheckpoint {
        lease: claim.lease.clone(),
        phase: DatabaseCheckpointPhase::Applied,
        cnpg_uid: uid.clone(),
        ca_certificate: ca,
        previous_pod_uid: None,
        migration_job_uid: None,
        copy_sha256: None,
    };
    let tx = hosting.begin_authenticated_agent(agent).await.unwrap();
    service
        .validate_database_checkpoint_in(&tx, agent, &claim.spec, &cp)
        .await
        .unwrap();
    service
        .checkpoint_database_operation(tx, agent, cp.clone())
        .await
        .unwrap();
    let configure = service
        .claim_database_operation(
            hosting.begin_authenticated_agent(agent).await.unwrap(),
            agent,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(configure.spec.phase, DatabaseExecutionPhase::Configure);
    cp.ca_certificate = None;
    for phase in [
        DatabaseCheckpointPhase::Configuring,
        DatabaseCheckpointPhase::Observing,
    ] {
        cp.phase = phase;
        if phase == DatabaseCheckpointPhase::Observing && action == DatabaseAction::Migrate {
            cp.migration_job_uid = Some("synthetic-migration-job".into());
            cp.copy_sha256 = Some("cd".repeat(32));
        }
        let tx = hosting.begin_authenticated_agent(agent).await.unwrap();
        service
            .validate_database_checkpoint_in(&tx, agent, &configure.spec, &cp)
            .await
            .unwrap();
        service
            .checkpoint_database_operation(tx, agent, cp.clone())
            .await
            .unwrap();
    }
    let observed = service
        .claim_database_operation(
            hosting.begin_authenticated_agent(agent).await.unwrap(),
            agent,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observed.spec.phase, DatabaseExecutionPhase::Observe);
    let completion = DatabaseCompletion {
        lease: claim.lease,
        outcome: DatabaseCompletionOutcome::Succeeded,
        proof: Some(DatabaseCompletionProof {
            installation_id: claim.spec.backend.id,
            database_id: claim.spec.database.as_ref().map(|d| d.id.clone()),
            cnpg_uid: uid,
            revision: claim.spec.revision,
            state: if action == DatabaseAction::Purge {
                DatabaseProofState::Purged
            } else {
                DatabaseProofState::Ready
            },
            endpoint_ready: action != DatabaseAction::Purge,
            copy_sha256: cp.copy_sha256,
            backup: None,
        }),
    };
    if action == DatabaseAction::Migrate {
        let mut forged = completion.clone();
        forged.proof.as_mut().unwrap().copy_sha256 = Some("00".repeat(32));
        assert!(
            service
                .complete_database_operation(
                    hosting.begin_authenticated_agent(agent).await.unwrap(),
                    agent,
                    forged
                )
                .await
                .is_err()
        );
    }
    let tx = hosting.begin_authenticated_agent(agent).await.unwrap();
    service
        .validate_database_callback_in(
            &tx,
            agent,
            &observed.spec,
            &observed.lease,
            DatabaseCallback::Completion(completion.outcome),
        )
        .await
        .unwrap();
    let response = service
        .complete_database_operation(tx, agent, completion.clone())
        .await
        .unwrap();
    (completion, response)
}

async fn callback_lock_order(
    owner: &sea_orm::DatabaseConnection,
    service: &DatabaseService,
    hosting: &spitzli_hosting::HostingService,
    agent: &spitzli_hosting::AgentIdentity,
    claim: &spitzli_databases::DatabaseClaim,
    checkpoint: Option<&spitzli_databases::DatabaseCheckpoint>,
) {
    let held = owner.begin().await.unwrap();
    held.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
        [format!("hosting-cluster:{}", agent.cluster_id()).into()],
    ))
    .await
    .unwrap();
    let callback = hosting.begin_authenticated_agent(agent).await.unwrap();
    {
        let validation = async {
            if let Some(cp) = checkpoint {
                service
                    .validate_database_checkpoint_in(&callback, agent, &claim.spec, cp)
                    .await
            } else {
                service
                    .validate_database_callback_in(
                        &callback,
                        agent,
                        &claim.spec,
                        &claim.lease,
                        spitzli_databases::DatabaseCallback::Guard,
                    )
                    .await
            }
        };
        tokio::pin!(validation);
        tokio::select! {result=&mut validation=>panic!("callback took operation rows before cluster lock: {}",result.is_ok()),_ = tokio::time::sleep(std::time::Duration::from_millis(80))=>{}}
        held.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM webdock_auth.hosting_operation WHERE id=$1 FOR UPDATE NOWAIT",
            [claim.spec.operation_id.clone().into()],
        ))
        .await
        .expect("blocked callback must not hold the operation row");
        held.commit().await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), validation)
            .await
            .expect("callback resumes after cluster unlock")
            .unwrap();
    }
    callback.rollback().await.unwrap();
}

async fn retired_order_service(cluster: &str) -> DatabaseService {
    let auth_url = guarded("AUTH_DATABASE_URL");
    let auth = Database::connect(auth_url.clone()).await.unwrap();
    let secret = std::env::var("BETTER_AUTH_SECRET").unwrap();
    let ids = Arc::new(
        spitzli_persistence::IdAllocator::connect(
            &auth_url,
            spitzli_ids::Identity::platform(0, 2).unwrap(),
        )
        .await
        .unwrap(),
    );
    let identity = Arc::new(IdentityService::new(
        auth.clone(),
        CookiePolicy::new(false, secret.clone()).unwrap(),
    ));
    DatabaseService::new(
        Database::connect(guarded("DATABASE_URL")).await.unwrap(),
        auth,
        Database::connect(guarded("WEBDOCK_PLUGIN_WORKER_DATABASE_URL"))
            .await
            .unwrap(),
        identity,
        None,
        ids,
        std::env::var("DATABASE_RLS_SECRET").unwrap(),
        &secret,
        "http://127.0.0.1:3150",
    )
    .await
    .unwrap()
    .with_new_orders_enabled(false)
    .with_management(spitzli_databases::ManagementConfig {
        cluster_id: Some(cluster.into()),
        endpoint_domain: Some("database.example.invalid".into()),
        public_ordering: false,
        protected_database_id: None,
        lease_secret: Some(secret),
    })
    .unwrap()
}

async fn saturated_order_and_agent_pools(
    actor: &spitzli_identity::Principal,
    authority: &StoredAuthority,
    command: &serde_json::Value,
    cluster: &str,
) {
    async fn limited(key: &str) -> sea_orm::DatabaseConnection {
        let mut options = sea_orm::ConnectOptions::new(guarded(key));
        options
            .max_connections(1)
            .min_connections(1)
            .acquire_timeout(std::time::Duration::from_millis(750));
        Database::connect(options).await.unwrap()
    }
    let auth = limited("AUTH_DATABASE_URL").await;
    let business = limited("DATABASE_URL").await;
    let worker = limited("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await;
    let secret = std::env::var("BETTER_AUTH_SECRET").unwrap();
    let rls = std::env::var("DATABASE_RLS_SECRET").unwrap();
    let ids = Arc::new(
        spitzli_persistence::IdAllocator::connect(
            &guarded("AUTH_DATABASE_URL"),
            spitzli_ids::Identity::platform(0, 2).unwrap(),
        )
        .await
        .unwrap(),
    );
    let identity = Arc::new(IdentityService::new(
        auth.clone(),
        CookiePolicy::new(false, secret.clone()).unwrap(),
    ));
    let hosting = spitzli_hosting::HostingService::new(
        business.clone(),
        auth.clone(),
        worker.clone(),
        rls.clone(),
        secret.clone(),
        ids.clone(),
    )
    .await
    .unwrap();
    let database = DatabaseService::new(
        business.clone(),
        auth.clone(),
        worker.clone(),
        identity,
        None,
        ids,
        rls,
        &secret,
        "http://127.0.0.1:3150",
    )
    .await
    .unwrap()
    .with_management(spitzli_databases::ManagementConfig {
        cluster_id: Some(cluster.into()),
        endpoint_domain: Some("database.example.invalid".into()),
        public_ordering: false,
        protected_database_id: None,
        lease_secret: Some(secret),
    })
    .unwrap();
    let agent = hosting
        .authenticate_agent(cluster, &format!("{cluster:0>43}"))
        .await
        .unwrap();
    let agent_tx = hosting.begin_authenticated_agent(&agent).await.unwrap();
    {
        let order = database.execute_managed(actor, authority, command.clone());
        tokio::pin!(order);
        tokio::select! {_=&mut order=>panic!("worker transaction is deliberately held by the callback"),_ = tokio::time::sleep(std::time::Duration::from_millis(80))=>{}}
        let callback = database.claim_database_operation(agent_tx, &agent);
        let (order, callback) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            tokio::join!(order, callback)
        })
        .await
        .expect("one connection per role must make progress");
        assert!(
            order.is_ok(),
            "accepted order must not exhaust a cyclic pool dependency: {}",
            order.err().map(|e| e.message).unwrap_or("none")
        );
        assert!(callback.unwrap().is_some());
    }
    drop(database);
    drop(hosting);
    auth.close().await.unwrap();
    business.close().await.unwrap();
    worker.close().await.unwrap();
}

// Compiled during the domain-only stage; executed only by the explicitly owned,
// ignored PostgreSQL fixture above. No synthetic Ready metadata is needed here.
async fn allocation_enqueue_failure_rolls_back(
    owner: &sea_orm::DatabaseConnection,
    actor: &spitzli_identity::Principal,
    authority: &StoredAuthority,
    command: &serde_json::Value,
    cluster: &str,
) {
    let counts_sql = "SELECT jsonb_build_object('backends',(SELECT count(*) FROM webdock_auth.hosting_database_backend WHERE cluster_id=$1),'ports',(SELECT count(*) FROM webdock_auth.hosting_database_port WHERE cluster_id=$1),'orders',(SELECT count(*) FROM webdock_auth.hosting_database_order WHERE offer_id=$3),'databases',(SELECT count(*) FROM webdock_auth.hosting_database WHERE project_id=$2),'jobs',(SELECT count(*) FROM webdock_auth.hosting_database_job j JOIN webdock_auth.hosting_database_backend b ON b.id=j.backend_id WHERE b.cluster_id=$1),'operations',(SELECT count(*) FROM webdock_auth.hosting_operation WHERE cluster_id=$1),'receipts',(SELECT count(*) FROM webdock_auth.hosting_operation WHERE subject=$4 AND idempotency_key=$5)) AS value";
    let values = vec![
        cluster.into(),
        command["projectID"].as_str().unwrap().into(),
        command["offerID"].as_str().unwrap().into(),
        actor.user_id.clone().into(),
        command["idempotencyKey"].as_str().unwrap().into(),
    ];
    async fn counts(
        owner: &sea_orm::DatabaseConnection,
        sql: &str,
        values: Vec<sea_orm::Value>,
    ) -> serde_json::Value {
        owner
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                values,
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "value")
            .unwrap()
    }
    let before = counts(owner, counts_sql, values.clone()).await;
    assert_eq!(
        before["orders"], 0,
        "rollback regression requires a never-accepted offer"
    );
    let auth_url = guarded("AUTH_DATABASE_URL");
    let auth = Database::connect(auth_url.clone()).await.unwrap();
    let business = Database::connect(guarded("DATABASE_URL")).await.unwrap();
    let worker = Database::connect(guarded("WEBDOCK_PLUGIN_WORKER_DATABASE_URL"))
        .await
        .unwrap();
    let secret = std::env::var("BETTER_AUTH_SECRET").unwrap();
    let ids = Arc::new(
        spitzli_persistence::IdAllocator::connect(
            &auth_url,
            spitzli_ids::Identity::platform(0, 2).unwrap(),
        )
        .await
        .unwrap(),
    );
    let identity = Arc::new(
        IdentityService::new(
            auth.clone(),
            CookiePolicy::new(false, secret.clone()).unwrap(),
        )
        .with_secret_keys(
            SecretKeys::new(
                BTreeMap::from([(0, secret.clone())]),
                0,
                Some(secret.clone()),
            )
            .unwrap(),
        )
        .with_allocator(ids.clone()),
    );
    let database = DatabaseService::new(
        business.clone(),
        auth.clone(),
        worker.clone(),
        identity,
        None,
        ids,
        std::env::var("DATABASE_RLS_SECRET").unwrap(),
        &secret,
        "http://127.0.0.1:3150",
    )
    .await
    .unwrap()
    .with_management(spitzli_databases::ManagementConfig {
        cluster_id: Some(cluster.into()),
        endpoint_domain: Some("database.example.invalid".into()),
        public_ordering: false,
        protected_database_id: None,
        // Allocation succeeds first; enqueue must refuse without its signer.
        lease_secret: None,
    })
    .unwrap();
    let error = database
        .execute_managed(actor, authority, command.clone())
        .await
        .unwrap_err();
    assert_eq!(error.status, 503);
    drop(database);
    auth.close().await.unwrap();
    business.close().await.unwrap();
    worker.close().await.unwrap();
    let after = counts(owner, counts_sql, values).await;
    assert_eq!(
        after, before,
        "failed enqueue must roll back every target reservation and row"
    );
}
