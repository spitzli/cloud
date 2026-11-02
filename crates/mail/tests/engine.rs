#![cfg(feature = "test-support")]
//! Real Community Edition engine, isolated local Docker network, only loopback published ports.
//! All identities, domains, passwords and volumes are generated for this disposable fixture.
#[path = "support/engine.rs"]
mod support;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    process::Stdio,
    time::{Duration, Instant},
};
use support::{Engine, PASSWORD, basic};
use webdock_mail::{
    jmap::{EmailQuery, JmapClient, MailAddress, MailAttachment, MailDraft, ManagementClient},
    smtp::{SmtpOptions, submit},
};
#[tokio::test]
#[ignore = "requires local Docker and the pinned Community Edition engine image"]
async fn real_engine_draft_attachment_private_smtp_and_account_isolation() {
    let Engine {
        fixture: _fixture,
        origin,
        smtp,
        accounts,
    } = Engine::provision(&["alice", "bob"]).await;
    let alice = JmapClient::local_fixture(
        &origin,
        &basic("alice@customer.test", PASSWORD),
        &accounts[0],
        None,
    )
    .unwrap();
    let bob = JmapClient::local_fixture(
        &origin,
        &basic("bob@customer.test", PASSWORD),
        &accounts[1],
        None,
    )
    .unwrap();
    let identity = alice.identities().await.unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut uploaded = alice
        .upload(b"Native attachment bytes".to_vec(), "text/plain")
        .await
        .unwrap();
    uploaded["name"] = json!("note.txt");
    let attachment: MailAttachment = serde_json::from_value(uploaded).unwrap();
    let original = alice
        .create_draft(
            MailDraft {
                identity_id: identity.clone(),
                to: vec![],
                cc: vec![],
                bcc: vec![],
                subject: "Working draft".into(),
                text: "Initial".into(),
                in_reply_to: None,
                references: None,
                attachments: vec![],
            },
            None,
        )
        .await
        .unwrap();
    let draft = alice
        .create_draft(
            MailDraft {
                identity_id: identity.clone(),
                to: vec![MailAddress {
                    email: "bob@customer.test".into(),
                    name: None,
                }],
                cc: vec![],
                bcc: vec![],
                subject: "Native Rust fixture".into(),
                text: "A real isolated message.".into(),
                in_reply_to: None,
                references: None,
                attachments: vec![attachment],
            },
            Some(original["id"].as_str().unwrap()),
        )
        .await
        .unwrap();
    assert!(alice.email(original["id"].as_str().unwrap()).await.is_err());
    let exported = alice
        .export_draft(draft["id"].as_str().unwrap(), &identity)
        .await
        .unwrap();
    assert_eq!(exported.to, ["bob@customer.test"]);
    let certificate = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new("openssl")
            .args([
                "s_client",
                "-connect",
                &format!("127.0.0.1:{smtp}"),
                "-showcerts",
            ])
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    let cert = rustls_pemfile::certs(&mut std::io::Cursor::new(certificate.stdout))
        .next()
        .unwrap()
        .unwrap();
    let smtp = SmtpOptions::local_fixture(
        smtp,
        hex::encode(Sha256::digest(cert.as_ref())),
        "alice@customer.test".into(),
        PASSWORD.into(),
        Duration::from_secs(15),
    )
    .unwrap();
    let prepared = webdock_mail::message::prepare_message(
        &exported.raw,
        &webdock_mail::message::Sender {
            email: exported.sender.email,
            name: exported.sender.name,
        },
        &exported.to,
    )
    .unwrap();
    submit(&smtp, &prepared.from, &prepared.to, &prepared.raw)
        .await
        .unwrap();
    let until = Instant::now() + Duration::from_secs(20);
    let received = loop {
        let rows = bob.emails(EmailQuery::default()).await.unwrap();
        if let Some(row) = rows["list"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["subject"] == "Native Rust fixture")
        {
            break row["id"].as_str().unwrap().to_owned();
        }
        assert!(
            Instant::now() < until,
            "isolated local delivery did not arrive"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    let message = bob.email(&received).await.unwrap();
    let blob = message["attachments"][0]["blobId"].as_str().unwrap();
    assert_eq!(
        bob.download_attachment(&received, blob)
            .await
            .unwrap()
            .bytes,
        b"Native attachment bytes"
    );
    alice
        .file_sent(draft["id"].as_str().unwrap())
        .await
        .unwrap();
    let foreign = JmapClient::local_fixture(
        &origin,
        &basic("bob@customer.test", PASSWORD),
        &accounts[0],
        None,
    )
    .unwrap();
    assert!(foreign.mailboxes().await.is_err());
    let user_management =
        ManagementClient::local_fixture(&origin, &basic("alice@customer.test", PASSWORD)).unwrap();
    assert!(
        user_management
            .call("x:Account/get", json!({"ids":null}))
            .await
            .is_err()
    );
}
