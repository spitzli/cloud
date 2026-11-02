use chrono::{Duration, Utc};
use webdock_agent_protocol::mail::*;

fn claim() -> MailClaim {
    let material = MailMaterial::Credentials {
        bootstrap_password: Some("owned-bootstrap-only".into()),
        username: "webdock-admin".into(),
        password: "owned-permanent-only".into(),
    };
    MailClaim {
        spec: MailSpec {
            version: 1,
            operation_id: "123".into(),
            customer_id: "124".into(),
            cluster_id: "125".into(),
            revision: 1,
            agent_generation: 1,
            instance_key: "mail-124".into(),
            hostname: "mail-124.example.test".into(),
            image: format!("registry.example.test/mail@sha256:{}", "a".repeat(64)),
            resources: MailResources::FIXED,
            material_sha256: material.fingerprint().unwrap(),
            action: MailAction::Ensure,
        },
        lease: MailLease {
            lease_token: "11111111-1111-4111-8111-111111111111".into(),
        },
        lease_until: Utc::now() + Duration::seconds(60),
        material,
        checkpoint: None,
    }
}
#[test]
fn mail_claim_binds_customer_profile_credentials_and_live_lease() {
    let mut value = claim();
    assert!(value.validate(Utc::now()).is_ok());
    let MailMaterial::Credentials { password, .. } = &mut value.material else {
        panic!("credentials expected")
    };
    *password = "changed".into();
    assert!(value.validate(Utc::now()).is_err());
    let mut value = claim();
    value.spec.instance_key = "mail-999".into();
    assert!(value.validate(Utc::now()).is_err());
    let mut value = claim();
    value.spec.hostname = "attacker.test".into();
    assert!(value.validate(Utc::now()).is_err());
    let mut value = claim();
    value.spec.resources.volume_bytes = 1;
    assert!(value.validate(Utc::now()).is_err());
    let mut value = claim();
    value.lease_until = Utc::now() - Duration::seconds(1);
    assert!(value.validate(Utc::now()).is_err());
}
#[test]
fn callbacks_never_accept_credentials_or_ambiguous_success() {
    let value = serde_json::json!({"lease":{"leaseToken":"11111111-1111-4111-8111-111111111111"},"runtimeID":"owned","materialSha256":"a".repeat(64),"password":"secret"});
    assert!(serde_json::from_value::<MailCheckpoint>(value).is_err());
    let value = MailCompletion {
        lease: claim().lease,
        outcome: MailOutcome::Succeeded,
        proof: None,
    };
    assert!(value.validate().is_err());
}
#[test]
fn running_success_requires_a_submission_pin_while_suspension_may_omit_it() {
    let mut completion = MailCompletion {
        lease: claim().lease,
        outcome: MailOutcome::Succeeded,
        proof: Some(MailProof {
            runtime_id: "owned-runtime".into(),
            namespace: "wd-mail-124".into(),
            revision: 1,
            running: true,
            edition: MailEdition::Community,
            recovery_disabled: true,
            submission_certificate_sha256: None,
            private_ipv4: None,
        }),
    };
    assert!(
        completion.validate().is_err(),
        "running success without a TLS pin is not readiness"
    );
    completion
        .proof
        .as_mut()
        .unwrap()
        .submission_certificate_sha256 = Some("a".repeat(64));
    assert!(completion.validate().is_ok());
    completion
        .proof
        .as_mut()
        .unwrap()
        .submission_certificate_sha256 = Some("g".repeat(64));
    assert!(completion.validate().is_err());
    let proof = completion.proof.as_mut().unwrap();
    proof.running = false;
    proof.submission_certificate_sha256 = None;
    assert!(
        completion.validate().is_ok(),
        "retained suspension need not observe a running listener"
    );
}
