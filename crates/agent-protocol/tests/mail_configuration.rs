use chrono::{Duration, Utc};
use serde_json::json;
use webdock_agent_protocol::mail_configuration::*;

fn claim() -> ConfigurationClaim {
    let material:ConfigurationMaterial=serde_json::from_value(json!({
        "administrator":{"username":"webdock-admin@mail-123.example.test","password":"a".repeat(43)},
        "identities":{"401":{"username":"wd-401@mail-123.example.test","password":"b".repeat(43)}}
    })).unwrap();
    let spec:ConfigurationSpec=serde_json::from_value(json!({
        "version":1,"operationID":"101","customerID":"123","clusterID":"200","revision":"4",
        "serviceRevision":"2","agentGeneration":1,"mode":"apply",
        "target":{"instanceKey":"mail-123","hostname":"mail-123.example.test",
            "image":format!("registry.example.test/mail@sha256:{}","a".repeat(64)),
            "resources":{"cpuMillicores":500,"memoryBytes":1073741824u64,"volumeBytes":10737418240u64,"ephemeralBytes":268435456u64},
            "managementSha256":material.management_fingerprint().unwrap(),"certificateSha256":"c".repeat(64)},
        "materialSha256":material.fingerprint().unwrap(),
        "snapshot":{
            "domains":[{"id":"201","name":"example.test","providerID":null,"ownershipVerified":true}],
            "mailboxes":[{"id":"301","domainID":"201","localPart":"mail","quotaBytes":1048576,"providerID":null}],
            "identities":[{"id":"401","subject":"501","providerID":null,"enabled":true,"mailboxIDs":["301"]}],
            "grants":[{"mailboxID":"301","subject":"501","revision":"1","enabled":true}]
        }
    })).unwrap();
    ConfigurationClaim {
        spec,
        material,
        lease: webdock_agent_protocol::mail::MailLease {
            lease_token: "11111111-1111-4111-8111-111111111111".into(),
        },
        lease_until: Utc::now() + Duration::seconds(60),
        checkpoints: vec![],
    }
}
#[test]
fn configuration_binds_material_revisions_tenant_and_exact_effective_grants() {
    let original = claim();
    original.validate(Utc::now()).unwrap();
    let mut value = original.clone();
    value.spec.customer_id = "999".into();
    assert!(value.validate(Utc::now()).is_err());
    let mut value = original.clone();
    value.material.administrator.password = "changed".into();
    assert!(value.validate(Utc::now()).is_err());
    let mut value = original.clone();
    value.spec.snapshot.identities[0]
        .mailbox_ids
        .push("999".into());
    assert!(value.validate(Utc::now()).is_err());
    let mut value = original.clone();
    value.spec.snapshot.grants[0].enabled = false;
    assert!(value.validate(Utc::now()).is_err());
    let mut value = original.clone();
    value.spec.snapshot.mailboxes[0].quota_bytes = 8589934593;
    assert!(value.validate(Utc::now()).is_err());
    let mut value = original;
    value.lease_until = Utc::now() - Duration::seconds(1);
    assert!(value.validate(Utc::now()).is_err());
}
#[test]
fn restriction_cannot_create_resources_or_enable_permissions() {
    let mut value = claim();
    value.spec.mode = ConfigurationMode::Restrict;
    assert!(value.validate(Utc::now()).is_err());
    value.spec.snapshot.domains.clear();
    value.spec.snapshot.mailboxes.clear();
    value.spec.snapshot.grants[0].enabled = false;
    let identity = &mut value.spec.snapshot.identities[0];
    identity.enabled = false;
    identity.mailbox_ids.clear();
    identity.provider_id = Some("owned-user".into());
    value.material.identities.clear();
    value.spec.material_sha256 = value.material.fingerprint().unwrap();
    value.validate(Utc::now()).unwrap();
    value.spec.snapshot.identities[0].provider_id = None;
    assert!(value.validate(Utc::now()).is_err());
}
#[test]
fn callbacks_cannot_carry_credentials_or_unbound_success() {
    let input = json!({"lease":{"leaseToken":"11111111-1111-4111-8111-111111111111"},"runtimeID":"owned-runtime",
        "phase":{"kind":"bound"},"password":"no"});
    assert!(serde_json::from_value::<ConfigurationCheckpoint>(input).is_err());
    let value = ConfigurationCompletion {
        lease: claim().lease,
        outcome: webdock_agent_protocol::mail::MailOutcome::Succeeded,
        proof: None,
    };
    assert!(value.validate().is_err());
}
#[test]
fn checkpoint_history_cannot_skip_handoff_change_runtime_or_include_foreign_resources() {
    let mut value = claim();
    let checkpoint = ConfigurationCheckpoint {
        lease: value.lease.clone(),
        runtime_id: "owned".into(),
        phase: ConfigurationCheckpointPhase::Applied {
            resource: ConfigurationResource {
                kind: ConfigurationResourceKind::Domain,
                id: "201".into(),
            },
            request_sha256: "a".repeat(64),
            provider_id: "owned-domain".into(),
        },
    };
    value.checkpoints.push(checkpoint);
    assert!(value.validate(Utc::now()).is_err());
    assert!(!ConfigurationMode::Restrict.advances_configuration());
}
