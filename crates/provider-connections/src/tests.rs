use super::*;
#[test]
fn strict_commands_and_revision_bounds() {
    for value in [
        json!({"action":"get","customerID":"1","apiKey":"fixture"}),
        json!({"action":"get","customerID":"01"}),
        json!({"action":"get","customerID":"9223372036854775808"}),
        json!({"action":"connect","customerID":"1","apiKey":"fixture"}),
        json!({"action":"connect","customerID":"1","apiKey":"fixture","expectedRevision":0,"idempotencyKey":"short"}),
        json!({"action":"disconnect","customerID":"1","expectedRevision":-1,"idempotencyKey":"fixture-key-12345"}),
        json!({"action":"disconnect","customerID":"1","expectedRevision":9007199254740991i64,"idempotencyKey":"fixture-key-12345"}),
        json!({"action":"projects","customerID":"1","cursor":"bad\ncursor"}),
        json!({"action":"projects","customerID":"1","organizationID":"org-other"}),
    ] {
        assert!(Command::parse(value).is_err());
    }
    assert!(Command::parse(json!({"action":"connect","customerID":"1","apiKey":"fixture","neonProjectID":"../wrong","expectedRevision":0,"idempotencyKey":"fixture-scoped-12345"})).is_err());
    assert!(Command::from_slice(br#"{"action":"get","customerID":"1","customerID":"2"}"#).is_err());
    assert!(Command::parse(json!({"action":"connect","customerID":"1","apiKey":"fixture","expectedRevision":0,"idempotencyKey":"fixture-key-12345"})).is_ok());
}
#[test]
fn ciphertext_is_bound_to_customer_connection_and_revision() {
    let cipher = SecretCipher::new(&"fixture-retained-secret".repeat(3)).unwrap();
    let aad = purpose("1", "connection-one", 2, None);
    let sealed = cipher.seal(&"fixture-provider-key", &aad).unwrap();
    assert_eq!(
        cipher.open::<String>(&sealed, &aad).unwrap(),
        "fixture-provider-key"
    );
    for aad in [
        purpose("2", "connection-one", 2, None),
        purpose("1", "connection-two", 2, None),
        purpose("1", "connection-one", 3, None),
    ] {
        assert!(cipher.open::<String>(&sealed, &aad).is_err());
    }
}
#[test]
fn credential_digest_and_output_are_safe_and_specific() {
    assert_ne!(
        payload_digest("connect", 0, None, None, Some("a")),
        payload_digest("connect", 0, None, None, Some("b"))
    );
    assert_ne!(
        payload_digest("connect", 0, None, None, Some("a")),
        payload_digest("connect", 1, None, None, Some("a"))
    );
    assert_ne!(
        payload_digest("connect", 0, None, None, Some("a")),
        payload_digest("connect", 0, Some("org-one"), None, Some("a"))
    );
    assert_ne!(
        payload_digest("connect", 0, None, Some("fixture-tree-123"), Some("a")),
        payload_digest("connect", 0, None, None, Some("a"))
    );
    let cipher = SecretCipher::new(&"fixture-key".repeat(8)).unwrap();
    let envelope = cipher
        .seal(
            &"fixture",
            &purpose("1", "one", 1, Some("fixture-tree-123")),
        )
        .unwrap();
    assert!(
        cipher
            .open::<String>(&envelope, &purpose("1", "one", 1, Some("another-tree-123")))
            .is_err()
    );
    assert!(
        cipher
            .open::<String>(&envelope, &purpose("1", "one", 1, None))
            .is_err()
    );
    let result = reply(
        Some(
            &json!({"id":"one","revision":1,"status":"connected","encrypted_key":"private","customer_id":"hidden","organization_id":null,"connected_at":"2026-11-01T00:00:00Z"}),
        ),
        true,
    );
    let text = result.to_string();
    assert!(!text.contains("private"));
    assert!(!text.contains("hidden"));
    assert!(!text.contains("encrypted_key"));
}

fn fixture_url(key: &str) -> String {
    let raw = std::env::var(key).unwrap();
    let parsed = url::Url::parse(&raw).unwrap();
    assert_eq!(parsed.host_str(), Some("127.0.0.1"));
    assert_eq!(parsed.path(), "/webdock_rls_test");
    assert!(parsed.query().is_none());
    raw
}
async fn fixture_sql(db: &impl ConnectionTrait, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap();
}
async fn fixture_principal(
    identity: &IdentityService,
    owner: &DatabaseConnection,
    user: &str,
    role: &str,
) -> Principal {
    fixture_sql(owner,"INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES($1,$1,$2,true,$3,true,false)",vec![user.into(),format!("{user}@example.invalid").into(),role.into()]).await;
    fixture_sql(owner,"INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES($1,$1,$2,now()+interval '1 hour',now())",vec![format!("{user}-session").into(),user.into()]).await;
    let tx = owner.begin().await.unwrap();
    identity
        .authenticate_session_in(&tx, user, &format!("{user}-session"))
        .await
        .unwrap()
        .unwrap()
}
fn connect_command(customer: &str, expected: i64, key: &str) -> Command {
    Command::parse(json!({"action":"connect","customerID":customer,"apiKey":"fixture-provider-key","organizationID":"org-fixture","expectedRevision":expected,"idempotencyKey":key})).unwrap()
}
fn disconnect_command(customer: &str, expected: i64, key: &str) -> Command {
    Command::parse(json!({"action":"disconnect","customerID":customer,"expectedRevision":expected,"idempotencyKey":key})).unwrap()
}

#[tokio::test]
#[ignore = "requires a fresh owned restricted-role Core PostgreSQL fixture"]
async fn restricted_core_connections_authority_replay_cleanup_and_provider_races() {
    use axum::{Router, extract::State, routing::get};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    struct ProviderGate {
        reject: AtomicBool,
        pause: AtomicBool,
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
        requests: AtomicUsize,
    }
    async fn projects(
        State(gate): State<Arc<ProviderGate>>,
        headers: axum::http::HeaderMap,
    ) -> axum::response::Response {
        use axum::response::IntoResponse;
        assert_eq!(headers["authorization"], "Bearer fixture-provider-key");
        gate.requests.fetch_add(1, Ordering::SeqCst);
        if gate.reject.load(Ordering::SeqCst) {
            return (
                axum::http::StatusCode::UNAUTHORIZED,
                "fixture-provider-key private diagnostic",
            )
                .into_response();
        }
        if gate.pause.load(Ordering::SeqCst) {
            gate.entered.notify_one();
            gate.release.notified().await;
        }
        axum::Json(
            json!({"projects":[{"id":"fixture-tree-123","name":"Fixture project","region_id":"aws-eu-central-1","connection_uri":"postgres://never-return-this"}]}),
        ).into_response()
    }
    let owner = sea_orm::Database::connect(fixture_url("WEBDOCK_TEST_MIGRATION_DATABASE_URL"))
        .await
        .unwrap();
    let auth_url = fixture_url("AUTH_DATABASE_URL");
    let auth = sea_orm::Database::connect(&auth_url).await.unwrap();
    let runtime = sea_orm::Database::connect(fixture_url("DATABASE_URL"))
        .await
        .unwrap();
    let secret = std::env::var("BETTER_AUTH_SECRET").unwrap();
    let identity = Arc::new(IdentityService::new(
        auth.clone(),
        webdock_identity::CookiePolicy::new(false, &secret).unwrap(),
    ));
    let allocator = webdock_persistence::IdAllocator::connect(
        &auth_url,
        webdock_ids::Identity::platform(0, 2).unwrap(),
    )
    .await
    .unwrap();
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let mut customers = Vec::new();
    let mut organizations = Vec::new();
    for _ in 0..2 {
        let customer = allocator.allocate().await.unwrap().to_string();
        let organization = allocator.allocate().await.unwrap().to_string();
        fixture_sql(&owner,"WITH hint AS MATERIALIZED(SELECT set_config('webdock.organization_allocation',jsonb_build_object('customer_id',$1::text,'organization_id',$2::text)::text,true)) INSERT INTO webdock_admin.customers(id,name) SELECT $1,'Neon fixture' FROM hint",vec![customer.clone().into(),organization.clone().into()]).await;
        customers.push(customer);
        organizations.push(organization);
    }
    let user = fixture_principal(&identity, &owner, &format!("neon-owner-{nonce}"), "user").await;
    let admin = fixture_principal(&identity, &owner, &format!("neon-admin-{nonce}"), "user").await;
    let member =
        fixture_principal(&identity, &owner, &format!("neon-member-{nonce}"), "user").await;
    let operator = fixture_principal(
        &identity,
        &owner,
        &format!("neon-operator-{nonce}"),
        "operator",
    )
    .await;
    for (principal, role) in [(&user, "owner"), (&admin, "admin"), (&member, "member")] {
        fixture_sql(&owner,"INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES($1,$2,$3,$4,now())",vec![allocator.allocate().await.unwrap().to_string().into(),organizations[0].clone().into(),principal.user_id.clone().into(),role.into()]).await;
    }
    fixture_sql(&owner,"INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES($1,$2,$3,'operator',$2)",vec![allocator.allocate().await.unwrap().to_string().into(),operator.user_id.clone().into(),operator.email.clone().into()]).await;
    let gate = Arc::new(ProviderGate {
        reject: AtomicBool::new(false),
        pause: AtomicBool::new(false),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        requests: AtomicUsize::new(0),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/projects", get(projects))
        .route("/projects/fixture-tree-123",get(|headers:axum::http::HeaderMap|async move {
            assert_eq!(headers["authorization"],"Bearer fixture-provider-key");
            axum::Json(json!({"project":{"id":"fixture-tree-123","name":"Scoped fixture","region_id":"aws-eu-central-1","org_id":"org-fixture"}}))
        }))
        .with_state(gate.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut service = Connections::new(
        runtime.clone(),
        auth.clone(),
        identity.clone(),
        None,
        &std::env::var("DATABASE_RLS_SECRET").unwrap(),
        &secret,
    )
    .unwrap();
    service.provider = neon::Client::test_client(origin);
    let service = Arc::new(service);
    let proof = StoredAuthority::local_session(&user);
    let customer = &customers[0];
    for principal in [&user, &admin, &operator] {
        assert_eq!(
            service
                .execute(
                    principal,
                    &StoredAuthority::local_session(principal),
                    Command::Get {
                        customer: customer.clone()
                    }
                )
                .await
                .unwrap()["canManage"],
            true
        );
    }
    for (principal, target) in [
        (&user, &customers[1]),
        (&admin, &customers[1]),
        (&member, customer),
    ] {
        assert!(matches!(
            service
                .execute(
                    principal,
                    &StoredAuthority::local_session(principal),
                    Command::Get {
                        customer: target.clone()
                    }
                )
                .await,
            Err(Error::Forbidden)
        ));
    }
    let connected = service
        .execute(
            &user,
            &proof,
            connect_command(customer, 0, "fixture-connect-0001"),
        )
        .await
        .unwrap();
    assert_eq!(connected["connection"]["revision"], 1);
    assert!(!connected.to_string().contains("fixture-provider-key"));
    assert_eq!(
        service
            .execute(
                &user,
                &proof,
                connect_command(customer, 0, "fixture-connect-0001")
            )
            .await
            .unwrap(),
        connected
    );
    assert_eq!(
        gate.requests.load(Ordering::SeqCst),
        1,
        "replay must not use provider key again"
    );
    gate.reject.store(true, Ordering::SeqCst);
    let rejected = service
        .execute(
            &user,
            &proof,
            connect_command(customer, 1, "fixture-invalid-key"),
        )
        .await
        .unwrap_err();
    assert!(matches!(rejected, Error::InvalidCredentials));
    assert_eq!(rejected.status(), 400);
    assert!(!rejected.to_string().contains("fixture-provider-key"));
    assert_eq!(
        service
            .execute(
                &user,
                &proof,
                Command::Get {
                    customer: customer.clone()
                }
            )
            .await
            .unwrap()["connection"]["revision"],
        1
    );
    gate.reject.store(false, Ordering::SeqCst);
    let changed=Command::parse(json!({"action":"connect","customerID":customer,"apiKey":"different-key","expectedRevision":0,"idempotencyKey":"fixture-connect-0001"})).unwrap();
    assert!(matches!(
        service.execute(&user, &proof, changed).await,
        Err(Error::Conflict)
    ));
    let inventory = service
        .execute(
            &user,
            &proof,
            Command::Projects {
                customer: customer.clone(),
                cursor: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(inventory["projects"].as_array().unwrap().len(), 1);
    assert!(!inventory.to_string().contains("postgres://"));
    let disconnected = service
        .execute(
            &user,
            &proof,
            disconnect_command(customer, 1, "fixture-disconnect-01"),
        )
        .await
        .unwrap();
    assert_eq!(disconnected["connection"]["revision"], 2);
    assert_eq!(disconnected["connection"]["status"], "disconnected");
    assert_eq!(
        service
            .execute(
                &user,
                &proof,
                disconnect_command(customer, 1, "fixture-disconnect-01")
            )
            .await
            .unwrap(),
        disconnected
    );
    let ciphertext = owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT encrypted_key FROM webdock_auth.neon_connection WHERE customer_id=$1",
            [customer.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<Option<String>>("", "encrypted_key")
        .unwrap();
    assert!(ciphertext.is_none());
    assert!(matches!(
        service
            .execute(
                &user,
                &proof,
                connect_command(customer, 0, "fixture-connect-0001")
            )
            .await,
        Err(Error::Conflict)
    ));
    service
        .execute(
            &user,
            &proof,
            connect_command(customer, 2, "fixture-connect-0003"),
        )
        .await
        .unwrap();
    // Read-only preview may see state but never obtain inventory or mutate the key.
    fixture_sql(&owner,"INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id) VALUES($1,$2,$3,$4)",vec![operator.session_id.clone().into(),operator.user_id.clone().into(),customer.clone().into(),organizations[0].clone().into()]).await;
    let tx = auth.begin().await.unwrap();
    let preview = identity
        .authenticate_session_in(&tx, &operator.user_id, &operator.session_id)
        .await
        .unwrap()
        .unwrap();
    drop(tx);
    let preview_proof = StoredAuthority::local_session(&preview);
    assert!(preview.in_read_only_preview());
    assert_eq!(
        service
            .execute(
                &preview,
                &preview_proof,
                Command::Get {
                    customer: customer.clone()
                }
            )
            .await
            .unwrap()["canManage"],
        false
    );
    assert!(matches!(
        service
            .execute(
                &preview,
                &preview_proof,
                Command::Get {
                    customer: customers[1].clone()
                }
            )
            .await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        service
            .execute(
                &preview,
                &preview_proof,
                Command::Projects {
                    customer: customer.clone(),
                    cursor: None
                }
            )
            .await,
        Err(Error::Forbidden)
    ));
    assert!(matches!(
        service
            .execute(
                &preview,
                &preview_proof,
                disconnect_command(customer, 3, "preview-disconnect-1")
            )
            .await,
        Err(Error::Forbidden)
    ));
    // A mismatched retained customer scope cannot revive write authority.
    let readonly = StoredAuthority::CustomerSession {
        customer_scope: "not-live".into(),
        read_only: true,
    };
    assert!(matches!(
        service
            .execute(
                &user,
                &readonly,
                disconnect_command(customer, 3, "readonly-disconnect")
            )
            .await,
        Err(Error::Forbidden)
    ));
    gate.pause.store(true, Ordering::SeqCst);
    let (svc, identity2, user_id, session, customer2) = (
        service.clone(),
        identity.clone(),
        user.user_id.clone(),
        user.session_id.clone(),
        customer.clone(),
    );
    let pending = tokio::spawn(async move {
        let auth_tx = svc.auth.begin().await.unwrap();
        let actor = identity2
            .authenticate_session_in(&auth_tx, &user_id, &session)
            .await
            .unwrap()
            .unwrap();
        drop(auth_tx);
        svc.execute(
            &actor,
            &StoredAuthority::local_session(&actor),
            Command::Projects {
                customer: customer2,
                cursor: None,
            },
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), gate.entered.notified())
        .await
        .unwrap();
    service
        .execute(
            &user,
            &proof,
            disconnect_command(customer, 3, "fixture-disconnect-04"),
        )
        .await
        .unwrap();
    gate.release.notify_one();
    assert!(matches!(pending.await.unwrap(), Err(Error::Conflict)));
    // Validation in flight cannot overwrite a concurrent disconnect with an old revision.
    let (svc, identity2, user_id, session, customer2) = (
        service.clone(),
        identity.clone(),
        user.user_id.clone(),
        user.session_id.clone(),
        customer.clone(),
    );
    let pending = tokio::spawn(async move {
        let auth_tx = svc.auth.begin().await.unwrap();
        let actor = identity2
            .authenticate_session_in(&auth_tx, &user_id, &session)
            .await
            .unwrap()
            .unwrap();
        drop(auth_tx);
        svc.execute(
            &actor,
            &StoredAuthority::local_session(&actor),
            connect_command(&customer2, 4, "fixture-connect-race"),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), gate.entered.notified())
        .await
        .unwrap();
    service
        .execute(
            &user,
            &proof,
            disconnect_command(customer, 4, "fixture-disconnect-05"),
        )
        .await
        .unwrap();
    gate.release.notify_one();
    assert!(matches!(pending.await.unwrap(), Err(Error::Conflict)));
    // SQL authority independently rejects unsigned access; no bypass by obtaining the pool.
    let unsigned = runtime
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*)::bigint AS n FROM webdock_auth.neon_connection WHERE customer_id=$1",
            [customer.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", "n")
        .unwrap();
    assert_eq!(unsigned, 0);
    let state = service
        .execute(
            &user,
            &proof,
            Command::Get {
                customer: customer.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(state["connection"]["revision"], 5);
    assert_eq!(state["connection"]["status"], "disconnected");
    // A session revoked while provider IO runs cannot receive a late inventory page.
    gate.pause.store(false, Ordering::SeqCst);
    service
        .execute(
            &user,
            &proof,
            connect_command(customer, 5, "fixture-connect-before-revoke"),
        )
        .await
        .unwrap();
    gate.pause.store(true, Ordering::SeqCst);
    let (svc, identity2, user_id, session, customer2) = (
        service.clone(),
        identity.clone(),
        user.user_id.clone(),
        user.session_id.clone(),
        customer.clone(),
    );
    let pending = tokio::spawn(async move {
        let tx = svc.auth.begin().await.unwrap();
        let actor = identity2
            .authenticate_session_in(&tx, &user_id, &session)
            .await
            .unwrap()
            .unwrap();
        drop(tx);
        svc.execute(
            &actor,
            &StoredAuthority::local_session(&actor),
            Command::Projects {
                customer: customer2,
                cursor: None,
            },
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), gate.entered.notified())
        .await
        .unwrap();
    fixture_sql(
        &owner,
        "DELETE FROM webdock_auth.session WHERE id=$1",
        vec![user.session_id.clone().into()],
    )
    .await;
    gate.release.notify_one();
    assert!(matches!(pending.await.unwrap(), Err(Error::Forbidden)));
    service
        .execute(
            &admin,
            &StoredAuthority::local_session(&admin),
            disconnect_command(customer, 6, "fixture-admin-revoke-cleanup"),
        )
        .await
        .unwrap();
    service
        .execute(
            &operator,
            &StoredAuthority::local_session(&operator),
            disconnect_command(&customers[1], 0, "fixture-delete-tombstone"),
        )
        .await
        .unwrap_err();
    // The operator currently has a preview, so clear it only in this disposable fixture.
    fixture_sql(
        &owner,
        "DELETE FROM webdock_auth.studio_tenant_preview WHERE session_id=$1",
        vec![operator.session_id.clone().into()],
    )
    .await;
    let tx = auth.begin().await.unwrap();
    let operator = identity
        .authenticate_session_in(&tx, &operator.user_id, &operator.session_id)
        .await
        .unwrap()
        .unwrap();
    drop(tx);
    service
        .execute(
            &operator,
            &StoredAuthority::local_session(&operator),
            disconnect_command(&customers[1], 0, "fixture-delete-tombstone"),
        )
        .await
        .unwrap();
    assert!(
        owner
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM webdock_auth.neon_connection_receipt WHERE customer_id=$1",
                [customers[1].clone().into()]
            ))
            .await
            .is_err(),
        "receipt cannot be erased while parent exists"
    );
    fixture_sql(
        &owner,
        "UPDATE webdock_admin.customers SET status='archived' WHERE id=$1",
        vec![customers[1].clone().into()],
    )
    .await;
    fixture_sql(
        &owner,
        "DELETE FROM webdock_admin.customers WHERE id=$1",
        vec![customers[1].clone().into()],
    )
    .await;
    let remaining=owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT (SELECT count(*) FROM webdock_auth.neon_connection WHERE customer_id=$1)+(SELECT count(*) FROM webdock_auth.neon_connection_receipt WHERE customer_id=$1) AS n",[customers[1].clone().into()])).await.unwrap().unwrap().try_get::<i64>("","n").unwrap();
    assert_eq!(
        remaining, 0,
        "permanent customer deletion cleans native metadata"
    );
    let broad_requests = gate.requests.load(Ordering::SeqCst);
    let scoped_value = json!({"action":"connect","customerID":customer,"apiKey":"fixture-provider-key","organizationID":"org-fixture","neonProjectID":"fixture-tree-123","expectedRevision":7,"idempotencyKey":"fixture-scoped-connect"});
    let scoped = Command::parse(scoped_value.clone()).unwrap();
    let scoped_reply = service
        .execute(&admin, &StoredAuthority::local_session(&admin), scoped)
        .await
        .unwrap();
    assert_eq!(
        scoped_reply["connection"]["neonProjectID"],
        "fixture-tree-123"
    );
    assert_eq!(
        service
            .execute(
                &admin,
                &StoredAuthority::local_session(&admin),
                Command::parse(scoped_value.clone()).unwrap()
            )
            .await
            .unwrap(),
        scoped_reply
    );
    let mut changed_scope = scoped_value;
    changed_scope["neonProjectID"] = json!("another-tree-123");
    assert!(matches!(
        service
            .execute(
                &admin,
                &StoredAuthority::local_session(&admin),
                Command::parse(changed_scope).unwrap()
            )
            .await,
        Err(Error::Conflict)
    ));
    let inventory = service
        .execute(
            &admin,
            &StoredAuthority::local_session(&admin),
            Command::Projects {
                customer: customer.clone(),
                cursor: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(inventory["projects"].as_array().unwrap().len(), 1);
    assert_eq!(inventory["nextCursor"], Value::Null);
    assert!(matches!(
        service
            .execute(
                &admin,
                &StoredAuthority::local_session(&admin),
                Command::Projects {
                    customer: customer.clone(),
                    cursor: Some("any-page".into())
                }
            )
            .await,
        Err(Error::InvalidInput)
    ));
    assert_eq!(
        gate.requests.load(Ordering::SeqCst),
        broad_requests,
        "scoped validation and reads never use account inventory"
    );
    let retained = service
        .execute(
            &admin,
            &StoredAuthority::local_session(&admin),
            disconnect_command(customer, 8, "fixture-scoped-disconnect"),
        )
        .await
        .unwrap();
    assert_eq!(retained["connection"]["neonProjectID"], "fixture-tree-123");
    server.abort();
}

#[test]
fn native_identity_failure_is_unavailable_without_exposing_diagnostics() {
    assert!(matches!(
        super::authority_error(webdock_browser_session::Error::Identity(
            webdock_identity::IdentityError::Persistence
        )),
        super::Error::Unavailable
    ));
}
