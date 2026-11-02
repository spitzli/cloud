#![cfg(feature = "local-provider-test")]
mod common;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use webdock_mail_protocol::{LeaseClaims, runtime::Handler, wire::send_receipt::State};
use webdock_mail_relay::Relay;
fn claims() -> LeaseClaims {
    LeaseClaims {
        v: 1,
        purpose: "mail-send".into(),
        audience: "webdock.mail.runtime.v1".into(),
        id: "fixture".into(),
        plugin_id: "relay-fixture".into(),
        generation: 1,
        customer_id: "123".into(),
        job_id: "456".into(),
        source_id: "key:789".into(),
        transport_revision: 1,
        grant_revision: 1,
        configuration_revision: 0,
        descriptor_sha256: "a".repeat(64),
        binding_sha256: "b".repeat(64),
        worker_id: "worker".into(),
        worker_generation: 1,
        worker_credential_sha256: "c".repeat(64),
        metadata_sha256: "d".repeat(64),
        mime_sha256: "e".repeat(64),
        mime_bytes: 100,
        iat: 1000,
        exp: 1060,
    }
}
#[tokio::test]
async fn provider_uses_one_fixed_request_preserves_large_receipts_and_classifies_uncertainty() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let fixture = common::Fixture::start(move |headers, body| {
        let index = counter.fetch_add(1, Ordering::SeqCst);
        assert!(headers.contains("consumerkey: fixture-key\r\n"));
        assert!(headers.contains("consumersecret: fixture-secret\r\n"));
        assert!(!headers.contains("authorization:"));
        assert_eq!(body["reference_id"], "456");
        assert_eq!(body["from"], "sender@example.test");
        assert_eq!(body["to"], "a@example.test");
        assert_eq!(body["bcc"], "hidden@example.test");
        let mime = body["mime_raw"].as_str().unwrap();
        assert!(!mime.contains("Bcc:") && !mime.contains("forged"));
        match index {
            0 => (200, "{\"mid\":18446744073709551615}".into()),
            1 => (200, "{\"mid\":\"18446744073709551616\"}".into()),
            2 => (200, "{\"mid\":1.5}".into()),
            3 => (500, "private-provider-detail".into()),
            4 => (429, "private-provider-detail".into()),
            5 => (302, "private-provider-detail".into()),
            _ => (200, "x".repeat(65537)),
        }
    })
    .await;
    let relay = Relay::local_fixture(&fixture.origin).unwrap();
    let metadata = json!({"sender":{"email":"sender@example.test","name":"Sender"},"to":["a@example.test"],"cc":[],"bcc":["hidden@example.test"],"subject":"Message"});
    let raw=b"From: forged@example.test\r\nBcc: hidden@example.test\r\nX-Webdock-Submission: forged\r\nSubject: Message\r\n\r\nBody".to_vec();
    let account = fixture.configuration("fixture-key", "fixture-secret");
    for (index, expected) in [
        State::Accepted,
        State::Accepted,
        State::Unknown,
        State::Unknown,
        State::Rejected,
        State::Unknown,
        State::Unknown,
    ]
    .into_iter()
    .enumerate()
    {
        let result = relay
            .submit(&claims(), metadata.clone(), raw.clone(), account.clone())
            .await;
        assert_eq!(result.state, expected as i32);
        if index == 0 {
            assert_eq!(result.provider_id, "18446744073709551615");
        }
        if index == 1 {
            assert_eq!(result.provider_id, "18446744073709551616");
        }
        assert_eq!(calls.load(Ordering::SeqCst), index + 1);
    }
    let result = relay
        .submit(
            &claims(),
            metadata.clone(),
            raw.clone(),
            json!({"region":"wrong","consumerKey":"fixture-key","consumerSecret":"fixture-secret"}),
        )
        .await;
    assert_eq!(result.state, State::Rejected as i32);
    for field in ["origin", "rules", "addresses"] {
        let mut denied = account.clone();
        denied["binding"][field] = match field {
            "origin" => json!("https://outside.invalid/"),
            _ => json!([]),
        };
        assert_eq!(
            relay
                .submit(&claims(), metadata.clone(), raw.clone(), denied)
                .await
                .state,
            State::Rejected as i32
        );
    }
    let mut invalid = metadata;
    invalid["subject"] = json!("Subject\r\nInjected: value");
    assert_eq!(
        relay.submit(&claims(), invalid, raw, account).await.state,
        State::Rejected as i32
    );
    assert_eq!(calls.load(Ordering::SeqCst), 7);
}
