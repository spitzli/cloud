use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};
use std::{collections::BTreeMap, sync::Arc};
use webdock_access::*;
use webdock_identity::{
    CookiePolicy, IdentityService, PasswordAuthentication, RlsSigner, SecretKeys,
};
#[derive(Default)]
struct CaptureDelivery(std::sync::Mutex<Vec<String>>);
#[async_trait::async_trait]
impl webdock_identity::Delivery for CaptureDelivery {
    async fn deliver(
        &self,
        request: &webdock_identity::DeliveryRequest,
    ) -> std::result::Result<(), webdock_identity::DeliveryError> {
        self.0.lock().unwrap().push(request.url.clone());
        Ok(())
    }
}
fn guarded(key: &str) -> String {
    let raw = std::env::var(key).unwrap();
    let u = url::Url::parse(&raw).unwrap();
    assert!(["postgres", "postgresql"].contains(&u.scheme()));
    assert!([Some("localhost"), Some("127.0.0.1"), Some("[::1]")].contains(&u.host_str()));
    assert_eq!(u.path(), "/webdock_rls_test");
    assert!(u.query().is_none() && u.fragment().is_none());
    raw
}
fn cookie(s: &str) -> &str {
    s.split(';').next().unwrap()
}
#[tokio::test]
#[ignore = "requires owned retained-schema restricted fixture"]
async fn tenant_invites_roles_and_revocation_preserve_restricted_boundaries() {
    let offline = Database::connect(guarded("WEBDOCK_TEST_MIGRATION_DATABASE_URL"))
        .await
        .unwrap();
    offline
        .execute_unprepared(include_str!("../migrations/0001_access_authority.sql"))
        .await
        .unwrap();
    let auth_url = guarded("AUTH_DATABASE_URL");
    let auth = Database::connect(auth_url.clone()).await.unwrap();
    let business = Database::connect(guarded("DATABASE_URL")).await.unwrap();
    let allocator = Arc::new(
        webdock_persistence::IdAllocator::connect(
            &auth_url,
            webdock_ids::Identity::platform(0, 2).unwrap(),
        )
        .await
        .unwrap(),
    );
    let secret = std::env::var("BETTER_AUTH_SECRET").unwrap();
    let keys = || {
        SecretKeys::new(
            BTreeMap::from([(0, secret.clone())]),
            0,
            Some(secret.clone()),
        )
        .unwrap()
    };
    let setup_mail = Arc::new(CaptureDelivery::default());
    let identity = Arc::new(
        IdentityService::new(
            auth.clone(),
            CookiePolicy::new(false, secret.clone()).unwrap(),
        )
        .with_account_origin(url::Url::parse("https://auth.example.invalid").unwrap())
        .unwrap()
        .with_secret_keys(keys())
        .with_allocator(allocator.clone())
        .with_delivery(setup_mail.clone()),
    );
    let captured = Arc::new(CaptureDelivery::default());
    let access = AccessService::new(
        business,
        auth.clone(),
        identity.clone(),
        allocator.clone(),
        RlsSigner::new(
            std::env::var("DATABASE_RLS_SECRET").unwrap(),
            "webdock_auth_runtime",
        )
        .unwrap(),
        Arc::new(keys()),
        AccessConfig {
            auth_origin: url::Url::parse("https://auth.example.invalid").unwrap(),
            console_origins: vec![url::Url::parse("https://studio.example.invalid").unwrap()],
            allow_local_http: false,
        },
    )
    .unwrap()
    .with_delivery(captured.clone());
    let mut users = vec![];
    let mut cookies = vec![];
    for label in ["owner", "other", "recipient"] {
        let email = format!(
            "access-{label}-{}@example.invalid",
            allocator.allocate().await.unwrap()
        );
        let id = identity
            .create_local_account(&email, "Access fixture", "fixture-password-123", false)
            .await
            .unwrap();
        auth.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.\"user\" SET \"emailVerified\"=true,\"mustChangePassword\"=false WHERE id=$1",[id.clone().into()])).await.unwrap();
        let PasswordAuthentication::Authenticated(s) = identity
            .authenticate_password(&email, "fixture-password-123")
            .await
            .unwrap()
        else {
            panic!("sign in")
        };
        users.push((id, email));
        cookies.push(s.set_cookie);
    }
    let mut customers = vec![];
    let mut orgs = vec![];
    let mut members = vec![];
    for user in users.iter().take(2) {
        let id = allocator.allocate().await.unwrap().to_string();
        offline
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO webdock_admin.customers(id,name) VALUES($1,'Access fixture')",
                [id.clone().into()],
            ))
            .await
            .unwrap();
        let org: String = offline
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
                [id.clone().into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "organization_id")
            .unwrap();
        let member = allocator.allocate().await.unwrap().to_string();
        offline.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES($1,$2,$3,'owner',now())",[member.clone().into(),org.clone().into(),user.0.clone().into()])).await.unwrap();
        customers.push(id);
        orgs.push(org);
        members.push(member);
    }
    assert_eq!(
        access
            .list_tenants(cookie(&cookies[0]))
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        access
            .get_tenant(cookie(&cookies[0]), &customers[1])
            .await
            .is_err()
    );
    assert!(
        access
            .manage_tenant(
                cookie(&cookies[0]),
                &customers[0],
                TenantCommand::Role {
                    member: members[0].clone(),
                    role: MemberRole::Member
                }
            )
            .await
            .is_err()
    );
    let invite = access
        .manage_tenant(
            cookie(&cookies[0]),
            &customers[0],
            TenantCommand::Invite {
                email: users[2].1.clone(),
                name: "Recipient".into(),
                role: MemberRole::Admin,
            },
        )
        .await
        .unwrap();
    let invitation = invite.id.unwrap();
    assert_eq!(invite.delivery, Some("queued"));
    assert!(
        invite.link.is_none(),
        "only operators receive a copyable link"
    );
    // A new person gets one setup mail that carries the invitation, never a throwaway password.
    let newcomer = format!(
        "access-new-{}@example.invalid",
        allocator.allocate().await.unwrap()
    );
    let fresh = access
        .manage_tenant(
            cookie(&cookies[0]),
            &customers[0],
            TenantCommand::Invite {
                email: newcomer.clone(),
                name: "Newcomer".into(),
                role: MemberRole::Admin,
            },
        )
        .await
        .unwrap();
    let fresh_invitation = fresh.id.unwrap();
    assert_eq!(fresh.delivery, Some("queued"));
    let separate: i64 = auth
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM webdock_auth.access_delivery_job WHERE invitation_id=$1",
            [fresh_invitation.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "n")
        .unwrap();
    assert_eq!(
        separate, 0,
        "the activation mail replaces the invitation mail"
    );
    let progress = |tenant: &serde_json::Value, id: &str| {
        tenant["invitations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == id)
            .cloned()
            .unwrap()
    };
    let before = access
        .get_tenant(cookie(&cookies[0]), &customers[0])
        .await
        .unwrap();
    let step = progress(&before, &fresh_invitation);
    assert_eq!(step["progress"]["emailDelivered"], "queued");
    assert_eq!(step["progress"]["accountReady"], false);
    assert!(step["progress"]["joinedAt"].is_null());
    assert!(
        before["activity"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["action"] == "tenant-invite"
                && event["actorEmail"] == users[0].1.as_str()),
        "the inviting owner sees their own invitation in the activity log"
    );
    identity.dispatch_delivery_jobs(100).await.unwrap();
    let mailed = access
        .get_tenant(cookie(&cookies[0]), &customers[0])
        .await
        .unwrap();
    assert_eq!(
        progress(&mailed, &fresh_invitation)["progress"]["emailDelivered"],
        "delivered"
    );
    let setup = setup_mail
        .0
        .lock()
        .unwrap()
        .iter()
        .find(|url| url.ends_with(&format!("&invitation={fresh_invitation}")))
        .cloned()
        .expect("activation link continues to the invitation");
    let token = url::Url::parse(&setup)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == "token")
        .unwrap()
        .1
        .into_owned();
    assert_eq!(
        identity
            .complete_password_reset(&token, "newcomer-password-123")
            .await
            .unwrap(),
        newcomer
    );
    let PasswordAuthentication::Authenticated(newcomer_session) = identity
        .authenticate_password(&newcomer, "newcomer-password-123")
        .await
        .unwrap()
    else {
        panic!("the new account signs in with the chosen password")
    };
    let joined = access
        .get_invitation(cookie(&newcomer_session.set_cookie), &fresh_invitation)
        .await
        .unwrap();
    assert_eq!(joined["customerID"], customers[0].as_str());
    let ready = access
        .get_tenant(cookie(&cookies[0]), &customers[0])
        .await
        .unwrap();
    let step = progress(&ready, &fresh_invitation);
    assert_eq!(step["progress"]["accountReady"], true);
    assert!(step["progress"]["joinedAt"].is_null());
    access
        .respond_invitation(
            cookie(&newcomer_session.set_cookie),
            &fresh_invitation,
            true,
        )
        .await
        .unwrap();
    let after = access
        .get_tenant(cookie(&cookies[0]), &customers[0])
        .await
        .unwrap();
    let step = progress(&after, &fresh_invitation);
    assert_eq!(step["status"], "accepted");
    assert!(step["progress"]["joinedAt"].is_string());
    // An expired invitation can be resent; a canceled one can be sent again to the same address.
    let late = format!(
        "access-late-{}@example.invalid",
        allocator.allocate().await.unwrap()
    );
    let late_invitation = access
        .manage_tenant(
            cookie(&cookies[0]),
            &customers[0],
            TenantCommand::Invite {
                email: late.clone(),
                name: "Late".into(),
                role: MemberRole::Member,
            },
        )
        .await
        .unwrap()
        .id
        .unwrap();
    offline
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_auth.invitation SET \"expiresAt\"=now()-interval '1 hour' WHERE id=$1",
            [late_invitation.clone().into()],
        ))
        .await
        .unwrap();
    let expired = access
        .get_tenant(cookie(&cookies[0]), &customers[0])
        .await
        .unwrap();
    assert_eq!(progress(&expired, &late_invitation)["expired"], true);
    access
        .manage_tenant(
            cookie(&cookies[0]),
            &customers[0],
            TenantCommand::Resend {
                invitation: late_invitation.clone(),
            },
        )
        .await
        .unwrap();
    let renewed = access
        .get_tenant(cookie(&cookies[0]), &customers[0])
        .await
        .unwrap();
    assert_eq!(progress(&renewed, &late_invitation)["expired"], false);
    access
        .manage_tenant(
            cookie(&cookies[0]),
            &customers[0],
            TenantCommand::Cancel {
                invitation: late_invitation.clone(),
            },
        )
        .await
        .unwrap();
    let again = access
        .manage_tenant(
            cookie(&cookies[0]),
            &customers[0],
            TenantCommand::Invite {
                email: late.clone(),
                name: "Late".into(),
                role: MemberRole::Member,
            },
        )
        .await
        .unwrap()
        .id
        .unwrap();
    assert_ne!(again, late_invitation);
    let repeated = access
        .get_tenant(cookie(&cookies[0]), &customers[0])
        .await
        .unwrap();
    assert_eq!(progress(&repeated, &late_invitation)["status"], "canceled");
    assert_eq!(progress(&repeated, &again)["status"], "pending");
    assert!(
        access
            .list_tenants(cookie(&cookies[2]))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        access
            .respond_invitation(cookie(&cookies[1]), &invitation, true)
            .await
            .is_err()
    );
    let accepted = access
        .respond_invitation(cookie(&cookies[2]), &invitation, true)
        .await
        .unwrap();
    let recipient_member = accepted["member"]["id"].as_str().unwrap().to_owned();
    assert!(
        matches!(
            access
                .manage_tenant(
                    cookie(&cookies[2]),
                    &customers[0],
                    TenantCommand::Invite {
                        email: "would-be-owner@example.invalid".into(),
                        name: "Would-be owner".into(),
                        role: MemberRole::Owner,
                    },
                )
                .await,
            Err(Error::Forbidden)
        ),
        "an administrator cannot hand out ownership"
    );
    assert_eq!(
        access
            .list_tenants(cookie(&cookies[2]))
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        access
            .manage_tenant(
                cookie(&cookies[2]),
                &customers[0],
                TenantCommand::Role {
                    member: members[0].clone(),
                    role: MemberRole::Member
                }
            )
            .await
            .is_err()
    );
    access
        .manage_tenant(
            cookie(&cookies[0]),
            &customers[0],
            TenantCommand::Role {
                member: recipient_member.clone(),
                role: MemberRole::Owner,
            },
        )
        .await
        .unwrap();
    access
        .manage_tenant(
            cookie(&cookies[0]),
            &customers[0],
            TenantCommand::Role {
                member: members[0].clone(),
                role: MemberRole::Member,
            },
        )
        .await
        .unwrap();
    assert!(
        access
            .manage_tenant(
                cookie(&cookies[0]),
                &customers[0],
                TenantCommand::Profile {
                    profile: Box::new(TenantProfileInput {
                        name: "Denied".into(),
                        ..Default::default()
                    })
                }
            )
            .await
            .is_err()
    );
    access
        .manage_tenant(
            cookie(&cookies[2]),
            &customers[0],
            TenantCommand::Remove {
                member: members[0].clone(),
            },
        )
        .await
        .unwrap();
    assert!(
        access
            .list_tenants(cookie(&cookies[0]))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        access
            .manage_access(
                cookie(&cookies[2]),
                AccessCommand::CreateOrganization {
                    name: "Forbidden".into()
                }
            )
            .await
            .is_err()
    );
    access.dispatch_invitation_jobs(100).await.unwrap();
    let state: String = auth
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT state FROM webdock_auth.access_delivery_job WHERE invitation_id=$1",
            [invitation.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "state")
        .unwrap();
    assert_eq!(state, "canceled", "answered invitation is not delivered");
    let count: i64 = offline
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM webdock_auth.access_event WHERE actor_id=$1",
            [users[0].0.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "n")
        .unwrap();
    assert!(count > 0);

    let operator_email = format!(
        "access-operator-{}@example.invalid",
        allocator.allocate().await.unwrap()
    );
    let operator = identity
        .create_local_account(
            &operator_email,
            "Operator fixture",
            "fixture-password-123",
            true,
        )
        .await
        .unwrap();
    auth.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE webdock_auth.\"user\" SET \"emailVerified\"=true WHERE id=$1",
        [operator.clone().into()],
    ))
    .await
    .unwrap();
    let PasswordAuthentication::SetupRequired {
        session: Some(setup),
        ..
    } = identity
        .authenticate_password(&operator_email, "fixture-password-123")
        .await
        .unwrap()
    else {
        panic!("operator setup")
    };
    assert!(
        access
            .manage_access(
                cookie(&setup.set_cookie),
                AccessCommand::CreateOrganization {
                    name: "Denied before MFA".into()
                }
            )
            .await
            .is_err()
    );
    let changed = identity
        .change_password(
            cookie(&setup.set_cookie),
            "fixture-password-123",
            "operator-password-456",
        )
        .await
        .unwrap();
    let enrollment = identity
        .begin_totp_enrollment(cookie(&changed.set_cookie), "operator-password-456")
        .await
        .unwrap();
    let totp = totp_rs::Totp::from_url(&enrollment.totp_uri).unwrap();
    let operator_session = identity
        .confirm_totp_enrollment(
            cookie(&changed.set_cookie),
            &totp.generate_current().to_string(),
        )
        .await
        .unwrap();
    let operator_cookie = cookie(&operator_session.set_cookie);
    // An operator invites the first owner on purpose; owners can make others owners.
    let empty = allocator.allocate().await.unwrap().to_string();
    offline
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO webdock_admin.customers(id,name) VALUES($1,'Access first owner')",
            [empty.clone().into()],
        ))
        .await
        .unwrap();
    let join = async |email: &str, session: &str, role| {
        let invitation = access
            .manage_tenant(
                operator_cookie,
                &empty,
                TenantCommand::Invite {
                    email: email.into(),
                    name: "Fixture".into(),
                    role,
                },
            )
            .await
            .unwrap()
            .id
            .unwrap();
        access
            .respond_invitation(cookie(session), &invitation, true)
            .await
            .unwrap()["member"]
            .clone()
    };
    let mut fresh_people = vec![];
    for label in ["first-owner", "second-owner"] {
        let email = format!(
            "access-{label}-{}@example.invalid",
            allocator.allocate().await.unwrap()
        );
        let id = identity
            .create_local_account(&email, "Access fixture", "fixture-password-123", false)
            .await
            .unwrap();
        auth.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.\"user\" SET \"emailVerified\"=true,\"mustChangePassword\"=false WHERE id=$1",[id.into()])).await.unwrap();
        let PasswordAuthentication::Authenticated(s) = identity
            .authenticate_password(&email, "fixture-password-123")
            .await
            .unwrap()
        else {
            panic!("sign in")
        };
        fresh_people.push((email, s.set_cookie));
    }
    let first = join(&fresh_people[0].0, &fresh_people[0].1, MemberRole::Owner).await;
    assert_eq!(
        first["role"], "owner",
        "an owner invitation makes the invitee owner"
    );
    let second = join(&fresh_people[1].0, &fresh_people[1].1, MemberRole::Admin).await;
    assert_eq!(second["role"], "admin", "members keep their invited role");
    access
        .manage_tenant(
            cookie(&fresh_people[0].1),
            &empty,
            TenantCommand::Role {
                member: second["id"].as_str().unwrap().into(),
                role: MemberRole::Owner,
            },
        )
        .await
        .expect("an owner can make another member an owner");
    access
        .manage_tenant(
            cookie(&fresh_people[0].1),
            &empty,
            TenantCommand::Role {
                member: first["id"].as_str().unwrap().into(),
                role: MemberRole::Admin,
            },
        )
        .await
        .expect("with a second owner the first owner can step down");
    assert!(
        !access
            .list_access(operator_cookie, "recipent")
            .await
            .unwrap()["users"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let created = access
        .manage_access(
            operator_cookie,
            AccessCommand::CreateOrganization {
                name: "Access standalone fixture".into(),
            },
        )
        .await
        .unwrap()
        .id
        .unwrap();

    let oauth = webdock_oauth::OAuthService::new(
        auth.clone(),
        identity.clone(),
        allocator.clone(),
        Arc::new(keys()),
        webdock_oauth::OAuthConfig::new(
            "https://auth.example.invalid/api/auth",
            "https://studio.example.invalid/api/mcp",
            None,
        )
        .unwrap(),
    );
    let client_record = oauth
        .provision_client(
            &offline,
            webdock_oauth::ClientRegistration {
                name: "Access fixture".into(),
                redirect_uris: vec!["https://site.example.invalid/api/sso/callback".into()],
                post_logout_redirect_uris: vec![],
                scopes: vec!["openid".into()],
                auth_method: webdock_oauth::ClientAuthMethod::None,
                allow_refresh: false,
                skip_consent: true,
                purpose: webdock_oauth::ClientPurpose::Organization {
                    organization_id: created.clone(),
                },
            },
        )
        .await
        .unwrap();
    let client = client_record.client_id;
    let binding = client_record.binding_id.unwrap();
    let oauth_request = || {
        webdock_oauth::AuthorizationRequest::code(
            client.clone(),
            "https://site.example.invalid/api/sso/callback".into(),
            "openid".into(),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        )
    };
    access
        .manage_access(
            operator_cookie,
            AccessCommand::LinkWebsite {
                binding: binding.clone(),
                organization: created.clone(),
            },
        )
        .await
        .unwrap();
    assert!(
        access
            .manage_access(
                operator_cookie,
                AccessCommand::LinkWebsite {
                    binding: binding.clone(),
                    organization: orgs[1].clone()
                }
            )
            .await
            .is_err()
    );
    access
        .manage_access(
            operator_cookie,
            AccessCommand::Invite {
                binding: binding.clone(),
                email: users[0].1.clone(),
                name: "Website recipient".into(),
                role: SiteRole::Reader,
            },
        )
        .await
        .unwrap();
    let invitation:String=auth.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM webdock_auth.invitation WHERE \"organizationId\"=$1 AND lower(email)=lower($2) AND status='pending'",[created.clone().into(),users[0].1.clone().into()])).await.unwrap().unwrap().try_get("","id").unwrap();
    access
        .respond_invitation(cookie(&cookies[0]), &invitation, true)
        .await
        .unwrap();
    let grant: String = auth
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM webdock_auth.project_grant WHERE binding_id=$1 AND user_id=$2",
            [binding.clone().into(), users[0].0.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "id")
        .unwrap();
    access
        .manage_access(
            operator_cookie,
            AccessCommand::ChangeAccess {
                grant: grant.clone(),
                role: SiteRole::Editor,
            },
        )
        .await
        .unwrap();
    assert!(
        !access
            .account_sites(cookie(&cookies[0]))
            .await
            .unwrap()
            .is_empty()
    );
    access
        .manage_access(
            operator_cookie,
            AccessCommand::RevokeAccess {
                grant: grant.clone(),
            },
        )
        .await
        .unwrap();
    assert!(
        access
            .account_sites(cookie(&cookies[0]))
            .await
            .unwrap()
            .is_empty()
    );
    access
        .start_preview(operator_cookie, &customers[0])
        .await
        .unwrap();
    assert!(
        access
            .manage_access(
                operator_cookie,
                AccessCommand::CreateOrganization {
                    name: "Denied in preview".into()
                }
            )
            .await
            .is_err()
    );
    assert_eq!(
        access.current_preview(operator_cookie).await.unwrap()["status"],
        "active"
    );
    offline.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.studio_tenant_preview SET expires_at=now()-interval '1 second' WHERE session_id=$1",[operator_session.principal.session_id.clone().into()])).await.unwrap();
    assert_eq!(
        access.current_preview(operator_cookie).await.unwrap()["status"],
        "expired"
    );
    assert!(access.require_operator(operator_cookie).await.is_err());
    access.exit_preview(operator_cookie).await.unwrap();
    access.require_operator(operator_cookie).await.unwrap();
    // Managed tenant policy and membership_role also reject stale native projections.
    offline.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.clerk_organization(issuer,clerk_organization_id,organization_id) VALUES('https://fixture.invalid',$1,$2)",[format!("managed-{}",orgs[1]).into(),orgs[1].clone().into()])).await.unwrap();
    assert!(
        access
            .list_tenants(cookie(&cookies[1]))
            .await
            .unwrap()
            .is_empty()
    );
    let principal = identity
        .authenticate_cookie(cookie(&cookies[1]))
        .await
        .unwrap()
        .unwrap();
    let runtime = Database::connect(guarded("DATABASE_URL")).await.unwrap();
    let signer = RlsSigner::new(
        std::env::var("DATABASE_RLS_SECRET").unwrap(),
        "webdock_auth_runtime",
    )
    .unwrap();
    let scoped = signer.begin(&runtime, &principal).await.unwrap();
    let row=scoped.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT webdock_rls.customer_allowed($1) AS allowed,webdock_rls.membership_role($2) AS role",[customers[1].clone().into(),orgs[1].clone().into()])).await.unwrap().unwrap();
    assert!(!row.try_get::<bool>("", "allowed").unwrap());
    assert!(row.try_get::<Option<String>>("", "role").unwrap().is_none());
    scoped.rollback().await.unwrap();
    let inv = access
        .inspect_organization_activation(&offline, operator_cookie, &orgs[1])
        .await
        .unwrap();
    access
        .activate_local_organization(
            &offline,
            operator_cookie,
            &orgs[1],
            vec![ApprovedMember {
                user_id: users[1].0.clone(),
                role: MemberRole::Owner,
            }],
            ActivationReview {
                snapshot_sha256: inv.snapshot_sha256,
                evidence_ref: "managed fixture reviewed owner".into(),
                evidence_sha256: "a".repeat(64),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        access
            .list_tenants(cookie(&cookies[1]))
            .await
            .unwrap()
            .len(),
        1
    );
    // Explicit offline transition: canonical subject and mapping tuple remain unchanged.
    let external = identity
        .create_local_account(
            &format!(
                "external-{}@example.invalid",
                allocator.allocate().await.unwrap()
            ),
            "External fixture",
            "old-password-123",
            false,
        )
        .await
        .unwrap();
    offline.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.clerk_identity(issuer,clerk_user_id,user_id) VALUES('https://fixture.invalid',$1,$2)",[format!("external-{external}").into(),external.clone().into()])).await.unwrap();
    let inv = access
        .inspect_identity_activation(&offline, operator_cookie, &external)
        .await
        .unwrap();
    let review = ActivationReview {
        snapshot_sha256: inv.snapshot_sha256,
        evidence_ref: "fixture reviewed bindings".into(),
        evidence_sha256: "a".repeat(64),
    };
    let mut stale = review.clone();
    stale.snapshot_sha256 = "b".repeat(64);
    assert!(
        access
            .activate_local_identity(
                &offline,
                operator_cookie,
                &external,
                LocalAccountRole::User,
                stale
            )
            .await
            .is_err()
    );
    let activated = access
        .activate_local_identity(
            &offline,
            operator_cookie,
            &external,
            LocalAccountRole::User,
            review,
        )
        .await
        .unwrap();
    assert_eq!(activated.delivery, Some("queued"));
    let archived: String = offline
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT archive_envelope FROM webdock_auth.local_identity_activation WHERE user_id=$1",
            [external.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "archive_envelope")
        .unwrap();
    let snapshot: serde_json::Value =
        serde_json::from_str(&keys().decrypt(&archived).unwrap()).unwrap();
    assert_eq!(snapshot["user"]["id"], external);
    assert!(
        auth.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT archive_envelope FROM webdock_auth.local_identity_activation WHERE user_id=$1",
            [external.clone().into()]
        ))
        .await
        .is_err()
    );

    let row=auth.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT \"emailVerified\",\"mustChangePassword\",webdock_rls.local_identity_authority(id) AS local FROM webdock_auth.\"user\" WHERE id=$1",[external.clone().into()])).await.unwrap().unwrap();
    assert!(row.try_get::<bool>("", "local").unwrap());
    assert!(!row.try_get::<bool>("", "emailVerified").unwrap());
    assert!(row.try_get::<bool>("", "mustChangePassword").unwrap());
    access
        .manage_access(
            operator_cookie,
            AccessCommand::ChangeAccess {
                grant: grant.clone(),
                role: SiteRole::Reader,
            },
        )
        .await
        .unwrap();
    assert!(
        oauth
            .authorize(cookie(&cookies[0]), oauth_request())
            .await
            .is_ok()
    );
    // An external organization activates only the explicit reviewed native owner manifest.
    offline.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.clerk_organization(issuer,clerk_organization_id,organization_id) VALUES('https://fixture.invalid',$1,$2)",[format!("external-{created}").into(),created.clone().into()])).await.unwrap();
    assert!(
        oauth
            .authorize(cookie(&cookies[0]), oauth_request())
            .await
            .is_err(),
        "native OAuth cannot trust external organization projections"
    );
    let principal = identity
        .authenticate_cookie(cookie(&cookies[0]))
        .await
        .unwrap()
        .unwrap();
    let signer = RlsSigner::new(
        std::env::var("DATABASE_RLS_SECRET").unwrap(),
        "webdock_auth_runtime",
    )
    .unwrap();
    let runtime = Database::connect(guarded("DATABASE_URL")).await.unwrap();
    let scoped = signer.begin(&runtime, &principal).await.unwrap();
    let allowed: bool = scoped
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT webdock_rls.organization_allowed($1) AS allowed",
            [created.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "allowed")
        .unwrap();
    assert!(!allowed, "signed native RLS must reject projected members");
    scoped.rollback().await.unwrap();
    let inv = access
        .inspect_organization_activation(&offline, operator_cookie, &created)
        .await
        .unwrap();
    access
        .activate_local_organization(
            &offline,
            operator_cookie,
            &created,
            vec![ApprovedMember {
                user_id: users[2].0.clone(),
                role: MemberRole::Owner,
            }],
            ActivationReview {
                snapshot_sha256: inv.snapshot_sha256,
                evidence_ref: "fixture organization review".into(),
                evidence_sha256: "a".repeat(64),
            },
        )
        .await
        .unwrap();
    assert!(
        oauth
            .authorize(cookie(&cookies[0]), oauth_request())
            .await
            .is_err(),
        "unapproved projected member remains revoked after activation"
    );
    access
        .manage_access(
            operator_cookie,
            AccessCommand::Invite {
                binding: binding.clone(),
                email: users[2].1.clone(),
                name: "Reviewed owner".into(),
                role: SiteRole::Reader,
            },
        )
        .await
        .unwrap();
    assert!(
        oauth
            .authorize(cookie(&cookies[2]), oauth_request())
            .await
            .is_ok(),
        "explicit native owner and newly approved site grant can authorize"
    );
    let delivered = access.dispatch_invitation_jobs(100).await.unwrap();
    assert!(delivered.delivered > 0);
    assert!(!captured.0.lock().unwrap().is_empty());
    let principal = identity
        .authenticate_cookie(cookie(&cookies[2]))
        .await
        .unwrap()
        .unwrap();
    let scoped = signer.begin(&runtime, &principal).await.unwrap();
    let role: Option<String> = scoped
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT webdock_rls.membership_role($1) AS role",
            [created.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "role")
        .unwrap();
    assert_eq!(role.as_deref(), Some("owner"));
    scoped.rollback().await.unwrap();
    assert!(
        access
            .manage_access(
                operator_cookie,
                AccessCommand::Suspend {
                    user: operator.clone(),
                    blocked: true
                }
            )
            .await
            .is_err()
    );
    access
        .manage_access(
            operator_cookie,
            AccessCommand::Suspend {
                user: users[0].0.clone(),
                blocked: true,
            },
        )
        .await
        .unwrap();
    assert!(
        identity
            .authenticate_cookie(cookie(&cookies[0]))
            .await
            .unwrap()
            .is_none()
    );
    for sql in [
        "DELETE FROM webdock_auth.local_identity_activation WHERE user_id=$1",
        "DELETE FROM webdock_auth.clerk_identity WHERE user_id=$1",
        "DELETE FROM webdock_auth.\"user\" WHERE id=$1",
    ] {
        offline
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                [external.clone().into()],
            ))
            .await
            .unwrap();
    }
    offline
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.project_grant WHERE binding_id=$1",
            [binding.clone().into()],
        ))
        .await
        .unwrap();
    offline
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.app_binding WHERE id=$1",
            [binding.into()],
        ))
        .await
        .unwrap();
    offline
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.\"oauthClient\" WHERE \"clientId\"=$1",
            [client.into()],
        ))
        .await
        .unwrap();
    for sql in [
        "DELETE FROM webdock_auth.local_organization_activation WHERE organization_id=$1",
        "DELETE FROM webdock_auth.clerk_organization WHERE organization_id=$1",
        "DELETE FROM webdock_auth.organization WHERE id=$1",
    ] {
        offline
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                [created.clone().into()],
            ))
            .await
            .unwrap();
    }
    auth.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM webdock_auth.\"user\" WHERE id=$1",
        [operator.into()],
    ))
    .await
    .unwrap();
    for sql in [
        "DELETE FROM webdock_auth.local_organization_activation WHERE organization_id=$1",
        "DELETE FROM webdock_auth.clerk_organization WHERE organization_id=$1",
    ] {
        offline
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                [orgs[1].clone().into()],
            ))
            .await
            .unwrap();
    }
    for customer in &customers {
        offline
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=$1",
                [customer.clone().into()],
            ))
            .await
            .unwrap();
    }
    for (user, _) in users {
        offline
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM webdock_auth.\"user\" WHERE id=$1",
                [user.into()],
            ))
            .await
            .unwrap();
    }
    for (customer, org) in customers.into_iter().zip(orgs) {
        offline
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM webdock_admin.customers WHERE id=$1",
                [customer.into()],
            ))
            .await
            .unwrap();
        offline
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM webdock_auth.organization WHERE id=$1",
                [org.into()],
            ))
            .await
            .unwrap();
    }
    auth.close().await.unwrap();
    offline.close().await.unwrap();
}
