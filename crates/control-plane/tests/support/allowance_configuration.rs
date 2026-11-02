//! Positive foundation uses only real bootstrap, PKCE, Registry and allowance APIs.
//! Every direct state mutation below is separately labeled adversarial/test deployment config.
use super::*;
use sea_orm::TransactionTrait;
#[path = "allowance_preview_races.rs"]
mod preview_races;

pub(super) struct Context<'a> {
    pub app: &'a Router,
    pub owner: &'a DatabaseConnection,
    pub auth: &'a DatabaseConnection,
    pub oauth: &'a Arc<webdock_oauth::OAuthService>,
    pub secrets: &'a Value,
    pub keys: &'a webdock_identity::SecretKeys,
    pub ids: &'a webdock_persistence::IdAllocator,
    pub business: &'a DatabaseConnection,
    pub native: &'a str,
    pub console: &'a str,
    pub identity: &'a Arc<webdock_identity::IdentityService>,
    pub access: &'a Arc<webdock_access::AccessService>,
    pub browser: &'a webdock_browser_session::BrowserSessions,
}
pub(super) async fn check_initial_mail_conflict(
    owner: &DatabaseConnection,
    environment: &HashMap<String, String>,
    offline: &url::Url,
    config: &Path,
    directory: &Path,
) {
    // NEGATIVE INSTALLATION CONFIGURATION TEST ONLY: no actors or allowances exist.
    sql(owner,"INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,'{\"mailEnabled\":false,\"retainedSetting\":7}')",vec![]).await;
    let output = directory.join("conflicting-initial-setting.json");
    assert!(matches!(
        webdock_control_plane::bootstrap::bootstrap_operator(
            environment,
            &target(offline),
            config,
            &output
        )
        .await,
        Err(webdock_control_plane::bootstrap::BootstrapError::Existing)
    ));
    assert!(!output.exists());
    assert_eq!(scalar(owner,"SELECT count(*)::bigint AS n FROM webdock_auth.platform_settings WHERE settings='{\"mailEnabled\":false,\"retainedSetting\":7}'::jsonb").await,1);
    assert_eq!(
        scalar(
            owner,
            "SELECT count(*)::bigint AS n FROM webdock_auth.\"user\""
        )
        .await,
        0
    );
    assert_eq!(
        scalar(
            owner,
            "SELECT count(*)::bigint AS n FROM webdock_auth.\"oauthClient\""
        )
        .await,
        0
    );
    // Remove only this negative setup row; positive bootstrap must set its own real value.
    sql(owner, "DELETE FROM webdock_auth.platform_settings", vec![]).await;
}

pub(super) async fn check_bootstrap_replay(
    owner: &DatabaseConnection,
    offline: &url::Url,
    config: &Path,
    directory: &Path,
) {
    let environment = HashMap::from([
        ("DATABASE_URL_UNPOOLED".into(), offline.as_str().to_owned()),
        ("WEBDOCK_AUTH_SECRET".into(), SECRET.to_owned()),
        ("BETTER_AUTH_URL".into(), AUTH.to_owned()),
        ("WEBDOCK_STUDIO_ORIGIN".into(), STUDIO.to_owned()),
        ("WEBDOCK_CONSOLE_ORIGIN".into(), CONSOLE.to_owned()),
    ]);
    assert_eq!(scalar(owner,"SELECT count(*)::bigint AS n FROM webdock_auth.platform_settings WHERE settings->'mailEnabled'='true'::jsonb").await,1);
    assert_eq!(scalar(owner,"SELECT count(*)::bigint AS n FROM webdock_auth.\"oauthClient\" WHERE metadata->'webdock_initial_setup'->'initialMailEnabled'='true'::jsonb").await,1);
    let original = std::fs::read(config).unwrap();
    let mut altered: Value = serde_json::from_slice(&original).unwrap();
    for enabled in [true, false] {
        altered["initialMailEnabled"] = json!(enabled);
        std::fs::write(config, serde_json::to_vec(&altered).unwrap()).unwrap();
        let output = directory.join(format!("replay-{enabled}.json"));
        assert!(matches!(
            webdock_control_plane::bootstrap::bootstrap_operator(
                &environment,
                &target(offline),
                config,
                &output
            )
            .await,
            Err(webdock_control_plane::bootstrap::BootstrapError::Existing)
        ));
        assert!(!output.exists());
        assert_eq!(scalar(owner,"SELECT count(*)::bigint AS n FROM webdock_auth.platform_settings WHERE settings->'mailEnabled'='true'::jsonb").await,1);
    }
    std::fs::write(config, original).unwrap();
    let shape:bool=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,
        "SELECT relrowsecurity AND relforcerowsecurity AND relowner='webdock_platform_owner'::regrole AS valid FROM pg_class WHERE oid='webdock_auth.allowance_operation'::regclass"))
        .await.unwrap().unwrap().try_get("","valid").unwrap();
    assert!(shape);
    assert_eq!(scalar(owner,"SELECT count(*)::bigint AS n FROM pg_policies WHERE schemaname='webdock_auth' AND tablename='allowance_operation'").await,3);
    let grants:bool=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,
        "SELECT has_table_privilege('webdock_auth_runtime','webdock_auth.allowance_operation','SELECT') AND has_table_privilege('webdock_auth_runtime','webdock_auth.allowance_operation','INSERT') AND NOT has_table_privilege('webdock_auth_runtime','webdock_auth.allowance_operation','UPDATE') AND NOT has_table_privilege('webdock_auth_runtime','webdock_auth.allowance_operation','DELETE') AND NOT has_table_privilege('webdock_worker_runtime','webdock_auth.allowance_operation','SELECT,INSERT,UPDATE,DELETE') AS valid"))
        .await.unwrap().unwrap().try_get("","valid").unwrap();
    assert!(grants);
}

fn command(revision: i32, key: &str) -> Value {
    json!({"expectedRevision":revision,"idempotencyKey":key,"hosting":{
        "apps":2,"cpuMillicores":1000,"memoryBytes":1073741824,
        "ephemeralBytes":2147483648u64,"volumeBytes":4294967296u64,
        "replicasPerApp":2,"concurrentDeployments":1},"mailMessages":10})
}
async fn configured(c: &Context<'_>, path: &str, body: Value) -> Reply {
    request(c.app, CONSOLE, "POST", path, c.console, body).await
}
async fn saved(c: &Context<'_>, customer: &str) -> Value {
    c.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT to_jsonb(s) AS value FROM webdock_auth.tenant_subscription s WHERE customer_id=$1",[customer.into()]))
        .await.unwrap().unwrap().try_get("", "value").unwrap()
}
async fn console_session(c: &Context<'_>) -> String {
    let begin = request(
        c.app,
        CONSOLE,
        "GET",
        "/api/sso/login?returnTo=%2Fprojects",
        "",
        Value::Null,
    )
    .await;
    assert!(begin.status.is_redirection());
    let authorize = begin.location();
    let granted = request(
        c.app,
        AUTH,
        "GET",
        &format!("{}?{}", authorize.path(), authorize.query().unwrap()),
        c.native,
        Value::Null,
    )
    .await;
    assert!(granted.status.is_redirection());
    let callback = granted.location();
    let issued = request(
        c.app,
        CONSOLE,
        "GET",
        &format!("{}?{}", callback.path(), callback.query().unwrap()),
        &begin.session(),
        Value::Null,
    )
    .await;
    assert!(issued.status.is_redirection());
    issued.session()
}
async fn assert_denied(c: &Context<'_>, path: &str, session: &str, body: Value) {
    let reply = request(c.app, CONSOLE, "POST", path, session, body).await;
    if reply.status == StatusCode::SERVICE_UNAVAILABLE {
        let category = match c.browser.resolve_authority(CONSOLE, session).await {
            Err(webdock_browser_session::Error::OAuth(webdock_oauth::Error::InvalidClient)) => {
                "invalid-client"
            }
            Err(webdock_browser_session::Error::OAuth(webdock_oauth::Error::AccessDenied)) => {
                "access-denied"
            }
            Err(webdock_browser_session::Error::Unauthorized) => "unauthorized",
            Err(webdock_browser_session::Error::Database(_)) => "database",
            Err(_) => "other-error",
            Ok(None) => "no-current-proof",
            Ok(Some(_)) => "browser-valid-domain-error",
        };
        eprintln!("native_allowance_503_boundary category={category}");
    }
    assert!(
        matches!(
            reply.status,
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
        ),
        "denied status {}",
        reply.status
    );
}
async fn token_revocation_fences_commit(c: &Context<'_>, customer: &str, path: &str, body: &Value) {
    eprintln!("native_allowance_phase token-fence-pkce");
    let session = console_session(c).await;
    let resolved = c
        .browser
        .resolve_authority(CONSOLE, &session)
        .await
        .unwrap()
        .unwrap();
    let encrypted: String = c
        .owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT payload FROM webdock_auth.identity_browser_session WHERE token_hash=$1",
            [resolved.authority.reference.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "payload")
        .unwrap();
    let tokens: Value = serde_json::from_str(&c.keys.decrypt(&encrypted).unwrap()).unwrap();
    let raw_token = tokens["access_token"].as_str().unwrap();
    let hash = webdock_oauth::legacy_token_hash(raw_token);
    let held = c.owner.begin().await.unwrap();
    held.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM webdock_admin.customers WHERE id=$1 FOR UPDATE",
        [customer.into()],
    ))
    .await
    .unwrap()
    .unwrap();
    let router = c.app.clone();
    let session_post = session.clone();
    let path_post = path.to_owned();
    let body_post = body.clone();
    let mutation = tokio::spawn(async move {
        request(
            &router,
            CONSOLE,
            "POST",
            &path_post,
            &session_post,
            body_post,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5),async {
        loop {
            let blocked:bool=c.business.query_one_raw(Statement::from_string(DbBackend::Postgres,
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%FOR UPDATE OF c FOR SHARE OF t%') AS blocked"))
                .await.unwrap().unwrap().try_get("","blocked").unwrap();
            if blocked {break;}
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.expect("allowance holds browser authority while waiting for customer");
    let form = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("client_id", c.secrets["clientID"].as_str().unwrap()),
            (
                "client_secret",
                c.secrets["environment"]["WEBDOCK_SSO_CLIENT_SECRET"]
                    .as_str()
                    .unwrap(),
            ),
            ("token", raw_token),
        ])
        .finish();
    let router = c.app.clone();
    let revoke = tokio::spawn(async move {
        router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/oauth2/revoke")
                    .header("host", "auth.example.test")
                    .header("origin", AUTH)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(form))
                    .unwrap(),
            )
            .await
            .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        !revoke.is_finished(),
        "canonical token remains locked through allowance commit"
    );
    held.commit().await.unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(5), mutation)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), revoke)
            .await
            .unwrap()
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let revoked:bool=c.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT revoked IS NOT NULL AS revoked FROM webdock_auth.\"oauthAccessToken\" WHERE token=$1",[hash.into()]))
        .await.unwrap().unwrap().try_get("","revoked").unwrap();
    assert!(revoked);
    assert_denied(c, path, &session, body.clone()).await;
}

async fn configured_authority_faults_remain_unavailable(c: &Context<'_>, path: &str) {
    // Genuine issued cookie, deliberately wrong protected server configuration.
    // No actor/session/token is constructed and no client secret is printed.
    let missing_client = c.ids.allocate().await.unwrap().to_string();
    let actual_client = c.secrets["clientID"].as_str().unwrap();
    let actual_secret = c.secrets["environment"]["WEBDOCK_SSO_CLIENT_SECRET"]
        .as_str()
        .unwrap();
    for (client, secret) in [
        (actual_client, "owned-incorrect-protected-client-secret-123"),
        (&missing_client, actual_secret),
    ] {
        let keys = webdock_identity::SecretKeys::new(
            BTreeMap::from([(1, SECRET.to_owned())]),
            1,
            Some(SECRET.to_owned()),
        )
        .unwrap();
        let misconfigured = Arc::new(
            webdock_browser_session::BrowserSessions::new(
                c.auth.clone(),
                Arc::clone(c.identity),
                Arc::clone(c.oauth),
                Arc::new(keys),
                c.secrets["environment"]["WEBDOCK_SSO_COOKIE_SECRET"]
                    .as_str()
                    .unwrap(),
                webdock_browser_session::BrowserConfig::new(
                    CONSOLE,
                    &format!("{AUTH}/api/auth"),
                    client.to_owned(),
                    secret.to_owned(),
                )
                .unwrap(),
            )
            .unwrap(),
        );
        assert!(matches!(
            misconfigured.resolve_authority(CONSOLE, c.console).await,
            Err(webdock_browser_session::Error::OAuth(
                webdock_oauth::Error::InvalidClient
            ))
        ));
        let app = build_app_with_domains(
            webdock_core::config::PlatformConfig::from_map(&HashMap::from([
                ("WEBDOCK_AUTH_ORIGIN".into(), AUTH.into()),
                ("WEBDOCK_STUDIO_ORIGIN".into(), STUDIO.into()),
                ("WEBDOCK_CONSOLE_ORIGIN".into(), CONSOLE.into()),
            ]))
            .unwrap(),
            c.auth.clone(),
            Services {
                identity: Some(Arc::clone(c.identity)),
                oauth: Some(Arc::clone(c.oauth)),
                browser: Some(misconfigured),
                access: Some(Arc::clone(c.access)),
                ..Default::default()
            },
        );
        assert_eq!(
            request(&app, CONSOLE, "GET", path, c.console, Value::Null)
                .await
                .status,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}

async fn injected_projected_contract(c: &Context<'_>, customer: &str) -> String {
    // ADVERSARIAL PAID-GUARD FAULT ONLY, AFTER REAL POSITIVE SETUP. No paid
    // subscription/payment exists: draft product mapping and disabled client below
    // deliberately model projected-contract state, not purchase or service activation.
    let mut ids = Vec::new();
    for _ in 0..5 {
        ids.push(c.ids.allocate().await.unwrap().to_string());
    }
    sql(c.owner,"INSERT INTO webdock_auth.billing_product(id,sku,name,family) VALUES($1,$1,'Test-only guard fault','website')",vec![ids[0].clone().into()]).await;
    sql(c.owner,"INSERT INTO webdock_auth.billing_product_version(id,product_id,version,currency,tax_behavior,\"interval\",description,entitlements,terms_hash) VALUES($1,$2,1,'CHF','inclusive','month','Test-only guard fault','{}',repeat('0',64))",vec![ids[1].clone().into(),ids[0].clone().into()]).await;
    sql(c.owner,"INSERT INTO webdock_auth.billing_product_mapping(environment,version_id) VALUES('live',$1)",vec![ids[1].clone().into()]).await;
    sql(c.owner,"INSERT INTO webdock_auth.\"oauthClient\"(id,\"clientId\",\"redirectUris\",disabled) VALUES($1,$1,'[]',true)",vec![ids[2].clone().into()]).await;
    sql(c.owner,"INSERT INTO webdock_auth.app_binding(id,client_id,label,organization_id,enabled) SELECT $1,$2,'Test-only guard fault',organization_id,false FROM webdock_auth.tenant_customer WHERE customer_id=$3",vec![ids[3].clone().into(),ids[2].clone().into(),customer.into()]).await;
    sql(c.owner,"INSERT INTO webdock_auth.billing_customer(environment,customer_id) VALUES('live',$1) ON CONFLICT DO NOTHING",vec![customer.into()]).await;
    let actor = c
        .identity
        .authenticate_cookie(c.native)
        .await
        .unwrap()
        .unwrap();
    sql(c.owner,"INSERT INTO webdock_auth.billing_contract(id,environment,customer_id,version_id,organization_id,binding_id,project_id,snapshot,terms_hash,accepted_by,state,service_status,projected_at) SELECT $1,'live',$2,$3,t.organization_id,$4,p.id,'{}',repeat('0',64),$5,'active','active',statement_timestamp() FROM webdock_auth.tenant_customer t JOIN webdock_admin.projects p ON p.customer_id=t.customer_id WHERE t.customer_id=$2 ORDER BY p.id LIMIT 1",vec![ids[4].clone().into(),customer.into(),ids[1].clone().into(),ids[3].clone().into(),actor.user_id.into()]).await;
    ids[4].clone()
}
async fn retained_usage(c: &Context<'_>, customer: &str) -> Value {
    c.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT jsonb_build_object('hosting',(SELECT jsonb_agg(to_jsonb(r) ORDER BY operation_id) FROM webdock_auth.hosting_reservation r WHERE customer_id=$1),'mail',(SELECT jsonb_agg(to_jsonb(r) ORDER BY reference_id) FROM webdock_mail.budget_reservation r WHERE customer_id=$1),'period',(SELECT to_jsonb(u) FROM webdock_mail.month_usage u WHERE customer_id=$1)) AS value",[customer.into()]))
        .await.unwrap().unwrap().try_get("","value").unwrap()
}
async fn lowering_retains_active_uncertain_usage(c: &Context<'_>, customer: &str, path: &str) {
    // ADVERSARIAL RETENTION STATE AFTER REAL POSITIVE SETUP. These metadata-only
    // reservations are not actual workload/sender/provider admission or delivery proof.
    let mut ids = Vec::new();
    for _ in 0..5 {
        ids.push(c.ids.allocate().await.unwrap().to_string());
    }
    sql(c.owner,"INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence) VALUES($1,'Test-only unverified retention fault','k3s','test','Test-only; no provider')",vec![ids[0].clone().into()]).await;
    sql(c.owner,"INSERT INTO webdock_auth.hosting_project(project_id,customer_id,provider,cluster_id,namespace,mode) SELECT id,customer_id,'k3s',$2,'test-retention','managed' FROM webdock_admin.projects WHERE customer_id=$1 ORDER BY id LIMIT 1",vec![customer.into(),ids[0].clone().into()]).await;
    let actor = c
        .identity
        .authenticate_cookie(c.native)
        .await
        .unwrap()
        .unwrap();
    for (operation, state) in [(&ids[1], "reserved"), (&ids[2], "active")] {
        sql(c.owner,"INSERT INTO webdock_auth.hosting_operation(id,subject,customer_id,project_id,cluster_id,action,idempotency_key,payload_hash,status,actor_session) SELECT $1,$2,$3,project_id,$4,'test-only-retention',$1,repeat('0',64),'needs-reconciliation',$5 FROM webdock_auth.hosting_project WHERE customer_id=$3",vec![operation.clone().into(),actor.user_id.clone().into(),customer.into(),ids[0].clone().into(),actor.session_id.clone().into()]).await;
        sql(c.owner,"INSERT INTO webdock_auth.hosting_reservation(operation_id,customer_id,project_id,cluster_id,demand,status) SELECT $1,customer_id,project_id,cluster_id,'{\"apps\":1,\"cpuMillicores\":100}', $2 FROM webdock_auth.hosting_project WHERE customer_id=$3",vec![operation.clone().into(),state.into(),customer.into()]).await;
    }
    sql(c.owner,"INSERT INTO webdock_mail.month_usage(customer_id,period_start,reserved,accepted) VALUES($1,date_trunc('month',now() AT TIME ZONE 'UTC')::date,2,3)",vec![customer.into()]).await;
    for (reference, state, count) in [(&ids[3], "reserved", 2), (&ids[4], "accepted", 3)] {
        sql(c.owner,"INSERT INTO webdock_mail.budget_reservation(reference_id,customer_id,period_start,units,state,send_path) VALUES($1,$2,date_trunc('month',now() AT TIME ZONE 'UTC')::date,$3,$4,'{\"kind\":\"direct\"}')",vec![reference.clone().into(),customer.into(),count.into(),state.into()]).await;
    }
    let before = retained_usage(c, customer).await;
    let mut lowered = command(3, "lower-preserves-usage");
    for value in lowered["hosting"].as_object_mut().unwrap().values_mut() {
        *value = json!(0);
    }
    lowered["mailMessages"] = json!(0);
    let changed = configured(c, path, lowered).await;
    assert_eq!(changed.status, StatusCode::OK);
    assert_eq!(changed.body["revision"], 4);
    assert_eq!(changed.body["effective"]["hosting"]["apps"], 0);
    assert_eq!(changed.body["effective"]["mailMessages"], 0);
    assert_eq!(retained_usage(c, customer).await, before);
}

pub(super) async fn check(c: Context<'_>) {
    if std::env::var("WEBDOCK_ALLOWANCE_PREVIEW_RACE_TEST").as_deref() == Ok("1") {
        preview_races::check(&c).await;
        return;
    }
    assert_eq!(
        scalar(
            c.owner,
            "SELECT count(*)::bigint AS n FROM webdock_admin.customers"
        )
        .await,
        0
    );
    assert_eq!(
        scalar(
            c.owner,
            "SELECT count(*)::bigint AS n FROM webdock_auth.tenant_subscription"
        )
        .await,
        0
    );
    // Loaded Hosting is activated by its real installation operator facade.
    assert_eq!(
        activate(c.app, c.console, HOSTING, "allowance-hosting")
            .await
            .status,
        StatusCode::OK
    );
    let customer = request(
        c.app,
        CONSOLE,
        "POST",
        "/api/console/registry/customers",
        c.console,
        json!({"name":"Native allowance qualification","country":"ch"}),
    )
    .await;
    assert_eq!(customer.status, StatusCode::OK);
    let customer = customer.body["id"].as_str().unwrap().to_owned();
    let path = format!("/api/console/access/customers/{customer}/allowances");
    let initial = request(c.app, CONSOLE, "GET", &path, c.console, Value::Null).await;
    assert_eq!(initial.status, StatusCode::OK);
    assert_eq!(initial.body["revision"], 0);
    assert_eq!(initial.body["effective"]["hosting"]["apps"], 0);
    assert_eq!(initial.body["effective"]["mailMessages"], 0);
    assert_eq!(
        scalar(
            c.owner,
            "SELECT count(*)::bigint AS n FROM webdock_auth.tenant_subscription"
        )
        .await,
        0
    );
    eprintln!("native_allowance_phase zero-before-grant");
    assert_denied(&c, &path, c.native, command(0, "bare-native-cookie")).await;
    let wrong_origin = request(
        c.app,
        STUDIO,
        "POST",
        &path,
        c.console,
        command(0, "wrong-app"),
    )
    .await;
    assert!(matches!(
        wrong_origin.status,
        StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED
    ));
    let body = command(0, "finite-first");
    let (first, same) = tokio::join!(
        configured(&c, &path, body.clone()),
        configured(&c, &path, body.clone())
    );
    assert_eq!(first.status, StatusCode::OK);
    assert_eq!(same.status, StatusCode::OK);
    assert_eq!(first.body, same.body);
    assert_eq!(first.body["revision"], 1);
    assert_eq!(first.body["effective"]["hosting"], body["hosting"]);
    assert_eq!(first.body["effective"]["mailMessages"], 10);
    assert!(first.body["receipt"]["operationID"].as_str().is_some());
    let sub = saved(&c, &customer).await;
    assert_eq!(sub["base"], json!({}));
    assert_eq!(sub["plan_id"], Value::Null);
    assert_eq!(
        scalar(
            c.owner,
            "SELECT count(*)::bigint AS n FROM webdock_auth.subscription_history"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(
            c.owner,
            "SELECT count(*)::bigint AS n FROM webdock_auth.allowance_operation"
        )
        .await,
        1
    );
    assert_eq!(scalar(c.owner,"SELECT count(*)::bigint AS n FROM webdock_auth.access_event WHERE action='allowances-configured'").await,1);
    assert_eq!(
        scalar(
            c.owner,
            "SELECT count(*)::bigint AS n FROM webdock_auth.billing_contract"
        )
        .await,
        0
    );
    let unread = c
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&path)
                .header("host", "console.example.test")
                .header("origin", CONSOLE)
                .header("cookie", c.console)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unread.status(), StatusCode::OK);
    drop(unread);
    configured_authority_faults_remain_unavailable(&c, &path).await;
    // Simulate a lost reply: discard the returned result and replay the exact command.
    drop(first);
    assert_eq!(configured(&c, &path, body.clone()).await.body, same.body);
    let mut changed = body.clone();
    changed["mailMessages"] = json!(11);
    assert_eq!(
        configured(&c, &path, changed).await.status,
        StatusCode::CONFLICT
    );
    assert_eq!(
        configured(&c, &path, command(0, "stale-revision"))
            .await
            .status,
        StatusCode::CONFLICT
    );
    let (a, b) = tokio::join!(
        configured(&c, &path, command(1, "race-a")),
        configured(&c, &path, command(1, "race-b"))
    );
    assert!(
        (a.status == StatusCode::OK && b.status == StatusCode::CONFLICT)
            || (b.status == StatusCode::OK && a.status == StatusCode::CONFLICT)
    );
    let current = request(c.app, CONSOLE, "GET", &path, c.console, Value::Null).await;
    assert_eq!(current.status, StatusCode::OK);
    assert_eq!(current.body["revision"], 2);
    assert_eq!(configured(&c, &path, body.clone()).await.body, same.body);
    let mut different = command(2, "body-race");
    different["mailMessages"] = json!(11);
    let (same_key_a, same_key_b) = tokio::join!(
        configured(&c, &path, command(2, "body-race")),
        configured(&c, &path, different)
    );
    assert!(
        (same_key_a.status == StatusCode::OK && same_key_b.status == StatusCode::CONFLICT)
            || (same_key_b.status == StatusCode::OK && same_key_a.status == StatusCode::CONFLICT)
    );
    let another = request(
        c.app,
        CONSOLE,
        "POST",
        "/api/console/registry/customers",
        c.console,
        json!({"name":"Native allowance second key scope"}),
    )
    .await;
    assert_eq!(another.status, StatusCode::OK);
    let other_customer = another.body["id"].as_str().unwrap().to_owned();
    let other_path = format!("/api/console/access/customers/{other_customer}/allowances");
    assert_eq!(
        configured(&c, &other_path, body.clone()).await.status,
        StatusCode::CONFLICT
    );
    for bad in [
        Value::Null,
        json!(-1),
        json!("unlimited"),
        json!(9007199254740992u64),
        json!(1.5),
    ] {
        let mut invalid = command(2, "invalid");
        invalid["mailMessages"] = bad;
        assert_eq!(
            configured(&c, &path, invalid).await.status,
            StatusCode::BAD_REQUEST
        );
    }
    for key in ["base", "planID", "role", "paid", "projection"] {
        let mut invalid = command(2, "invalid");
        invalid[key] = json!(true);
        assert_eq!(
            configured(&c, &path, invalid).await.status,
            StatusCode::BAD_REQUEST
        );
    }
    let mut invalid = command(2, "invalid");
    invalid["hosting"]["unknown"] = json!(1);
    assert_eq!(
        configured(&c, &path, invalid).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        configured(
            &c,
            &path.replace(&customer, "999999999"),
            command(0, "absent-customer")
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );

    // Restricted native runtime can read its receipt but cannot rewrite/delete it.
    let actor = c
        .identity
        .authenticate_cookie(c.native)
        .await
        .unwrap()
        .unwrap();
    let receipt_tx = webdock_identity::RlsSigner::new(SECRET, "webdock_auth_runtime")
        .unwrap()
        .begin(c.business, &actor)
        .await
        .unwrap();
    assert!(
        receipt_tx
            .execute_unprepared("DELETE FROM webdock_auth.allowance_operation")
            .await
            .is_err()
    );
    receipt_tx.rollback().await.unwrap();
    let mismatch = webdock_identity::RlsSigner::new(SECRET, "webdock_auth_runtime")
        .unwrap()
        .begin(c.business, &actor)
        .await
        .unwrap();
    assert!(mismatch.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,
        "INSERT INTO webdock_auth.allowance_operation(id,actor_id,customer_id,idempotency_key,payload_sha256,result) VALUES('mismatch','another-actor',$1,'mismatch',repeat('0',64),'{}')",
        [customer.clone().into()])).await.is_err());
    mismatch.rollback().await.unwrap();
    let unauthenticated = c.business.begin().await.unwrap();
    assert!(unauthenticated.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,
        "INSERT INTO webdock_auth.allowance_operation(id,actor_id,customer_id,idempotency_key,payload_sha256,result) VALUES('unauthenticated',$1,$2,'unauthenticated',repeat('0',64),'{}')",
        [actor.user_id.clone().into(),customer.clone().into()])).await.is_err());
    unauthenticated.rollback().await.unwrap();
    let immutable = c.owner.begin().await.unwrap();
    assert!(
        immutable
            .execute_unprepared("UPDATE webdock_auth.allowance_operation SET result='{}'")
            .await
            .is_err()
    );
    immutable.rollback().await.unwrap();

    eprintln!("native_allowance_phase positive-and-cas-complete");
    token_revocation_fences_commit(&c, &customer, &path, &body).await;
    eprintln!("native_allowance_phase token-fence-complete");

    lowering_retains_active_uncertain_usage(&c, &customer, &path).await;
    // ADVERSARIAL TEST STATE AFTER GENUINE SETUP: unrelated product fields and an
    // unlimited base characterize preservation; this is not a paid/live activation.
    sql(c.owner,"UPDATE webdock_auth.tenant_subscription SET base='{\"hosting\":{\"apps\":null},\"mailMessages\":null,\"projects\":5}',extras=jsonb_set(extras||'{\"projects\":3}','{hosting,legacyUnsupported}', '7'),name='Retained name',description='Retained description' WHERE customer_id=$1",vec![customer.clone().into()]).await;
    let contract = injected_projected_contract(&c, &customer).await;
    let contract_before: Value = c
        .owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT to_jsonb(c) AS value FROM webdock_auth.billing_contract c WHERE id=$1",
            [contract.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "value")
        .unwrap();
    let actor = c
        .identity
        .authenticate_cookie(c.native)
        .await
        .unwrap()
        .unwrap();
    let paid_guard = webdock_identity::RlsSigner::new(SECRET, "webdock_auth_runtime")
        .unwrap()
        .begin(c.business, &actor)
        .await
        .unwrap();
    assert!(
        paid_guard
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE webdock_auth.tenant_subscription SET base='{}' WHERE customer_id=$1",
                [customer.clone().into()]
            ))
            .await
            .is_err()
    );
    paid_guard.rollback().await.unwrap();
    let preserved = configured(&c, &path, command(4, "preserved-base")).await;
    assert_eq!(preserved.status, StatusCode::OK);
    assert_eq!(
        preserved.body["subscriptionEffective"]["hosting"]["apps"],
        Value::Null
    );
    assert_eq!(preserved.body["effective"]["mailMessages"], Value::Null);
    let sub = saved(&c, &customer).await;
    assert_eq!(
        sub["base"],
        json!({"hosting":{"apps":null},"mailMessages":null,"projects":5})
    );
    assert_eq!(sub["extras"]["projects"], 3);
    assert_eq!(sub["extras"]["hosting"]["legacyUnsupported"], 7);
    assert!(
        preserved.body["supplements"]["hosting"]
            .get("legacyUnsupported")
            .is_none()
    );
    assert_eq!(sub["name"], "Retained name");
    assert_eq!(sub["description"], "Retained description");

    let contract_after: Value = c
        .owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT to_jsonb(c) AS value FROM webdock_auth.billing_contract c WHERE id=$1",
            [contract.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "value")
        .unwrap();
    assert_eq!(contract_after, contract_before);
    injected_projected_contract(&c, &other_customer).await;
    assert_eq!(
        configured(&c, &other_path, command(0, "missing-projected-base"))
            .await
            .status,
        StatusCode::CONFLICT
    );
    assert_eq!(
        scalar(
            c.owner,
            "SELECT count(*)::bigint AS n FROM webdock_auth.tenant_subscription"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(
            c.owner,
            "SELECT count(*)::bigint AS n FROM webdock_auth.billing_manual_release"
        )
        .await,
        0
    );
    assert_eq!(
        scalar(
            c.owner,
            "SELECT count(*)::bigint AS n FROM webdock_auth.billing_subscription"
        )
        .await,
        0
    );

    eprintln!("native_allowance_phase paid-and-retained-usage-complete");
    // ADVERSARIAL: downgrade the mapping on the genuinely bootstrapped operator.
    sql(
        c.owner,
        "UPDATE webdock_admin.users SET role='reader'",
        vec![],
    )
    .await;
    assert_denied(&c, &path, c.console, body.clone()).await;
    sql(
        c.owner,
        "UPDATE webdock_admin.users SET role='operator'",
        vec![],
    )
    .await;
    eprintln!("native_allowance_phase mapping-revocation-complete");
    for mode in [
        webdock_access::PreviewMode::ReadOnly,
        webdock_access::PreviewMode::Owner,
    ] {
        let principal = c
            .identity
            .authenticate_cookie(c.native)
            .await
            .unwrap()
            .unwrap();
        c.access
            .start_customer_scope_as(&principal, &customer, mode)
            .await
            .unwrap();
        eprintln!("native_allowance_phase preview-denial mode={mode:?}");
        assert_denied(&c, &path, c.console, body.clone()).await;
        // Existing OAuth may send a relative account-selection/login redirect in
        // preview; no privileged browser proof is fabricated for that denial.
        c.access.exit_preview(c.native).await.unwrap();
    }
    eprintln!("native_allowance_phase preview-denials-complete");
    // ADVERSARIAL: customer archival invalidates both fresh writes and retained replay.
    sql(
        c.owner,
        "UPDATE webdock_admin.customers SET status='archived' WHERE id=$1",
        vec![customer.clone().into()],
    )
    .await;
    assert_denied(&c, &path, c.console, body.clone()).await;
    sql(
        c.owner,
        "UPDATE webdock_admin.customers SET status='active' WHERE id=$1",
        vec![customer.clone().into()],
    )
    .await;
    eprintln!("native_allowance_phase customer-revocation-complete");
    let fresh_console = console_session(&c).await;
    let live = c
        .browser
        .resolve_authority(CONSOLE, &fresh_console)
        .await
        .unwrap()
        .unwrap();
    // ADVERSARIAL: revoke the actual browser reference issued by PKCE, never seed one.
    sql(
        c.owner,
        "DELETE FROM webdock_auth.identity_browser_session WHERE token_hash=$1",
        vec![live.authority.reference.into()],
    )
    .await;
    assert_denied(&c, &path, &fresh_console, body.clone()).await;
    eprintln!("native_allowance_phase browser-revocation-complete");
    // ADVERSARIAL: current client revocation must deny a retained replay.
    sql(
        c.owner,
        "UPDATE webdock_auth.\"oauthClient\" SET disabled=true WHERE \"clientId\"=$1",
        vec![c.secrets["clientID"].as_str().unwrap().into()],
    )
    .await;
    assert_eq!(
        configured(&c, &path, body.clone()).await.status,
        StatusCode::UNAUTHORIZED
    );
    sql(
        c.owner,
        "UPDATE webdock_auth.\"oauthClient\" SET disabled=false WHERE \"clientId\"=$1",
        vec![c.secrets["clientID"].as_str().unwrap().into()],
    )
    .await;
    eprintln!("native_allowance_phase client-revocation-complete");
    // ADVERSARIAL: downgrade the real actor to a member/reader; no fabricated actor/session.
    let actor = c
        .identity
        .authenticate_cookie(c.native)
        .await
        .unwrap()
        .unwrap();
    sql(c.owner,"INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") SELECT $3,organization_id,$2,'member',statement_timestamp() FROM webdock_auth.tenant_customer WHERE customer_id=$1",vec![customer.clone().into(),actor.user_id.into(),c.ids.allocate().await.unwrap().to_string().into()]).await;
    sql(
        c.owner,
        "UPDATE webdock_auth.\"user\" SET role='user'",
        vec![],
    )
    .await;
    assert_denied(&c, &path, c.console, body.clone()).await;
    sql(
        c.owner,
        "UPDATE webdock_auth.\"user\" SET role='operator'",
        vec![],
    )
    .await;
    let principal = c
        .identity
        .authenticate_cookie(c.native)
        .await
        .unwrap()
        .unwrap();
    c.identity
        .revoke_session(&principal.user_id, &principal.session_id)
        .await
        .unwrap();
    assert_denied(&c, &path, c.console, body).await;
    assert_eq!(
        scalar(
            c.owner,
            "SELECT count(*)::bigint AS n FROM webdock_auth.allowance_operation"
        )
        .await,
        5
    );
    assert_eq!(
        scalar(
            c.owner,
            "SELECT count(*)::bigint AS n FROM webdock_auth.subscription_history"
        )
        .await,
        5
    );
    eprintln!("native_allowance_positive_api_and_adversarial_checks_complete customer={customer}");
}
