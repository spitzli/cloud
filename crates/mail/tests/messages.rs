use serde_json::{Value, json};
use webdock_mail::{message::*, proof::*, secrets::MailCipher};

#[test]
fn admitted_request_bytes_preserve_retained_idempotency_hashes() {
    use sha2::{Digest, Sha256};
    let cases: Vec<Value> = serde_json::from_str(include_str!("node-request.json")).unwrap();
    for case in cases {
        let request = SendRequest::parse(case["input"].clone(), chrono::Utc::now()).unwrap();
        let serialized = serde_json::to_string(&request).unwrap();
        assert_eq!(serialized, case["stored"].as_str().unwrap());
        assert_eq!(
            hex::encode(Sha256::digest(serialized.as_bytes())),
            case["hash"].as_str().unwrap()
        );
    }
}

#[test]
fn compilation_preserves_the_already_parsed_display_name() {
    let request = SendRequest::parse(json!({"from":"\"\"Quoted\"\" <sender@example.test>","to":"recipient@example.test","subject":"Name","text":"Body"}), chrono::Utc::now()).unwrap();
    assert_eq!(request.from.name.as_deref(), Some("\"Quoted\""));
    assert_eq!(request.compile().unwrap().sender.name, request.from.name);
}

#[test]
fn retained_node_proof_and_secret_are_compatible() {
    let fixture: Value = serde_json::from_str(include_str!("node-compatibility.json")).unwrap();
    let get = |name| fixture[name].as_str().unwrap();
    assert_eq!(
        message_digest(get("raw").as_bytes()).unwrap(),
        get("digest")
    );
    let context: SubmissionProof = serde_json::from_value(fixture["context"].clone()).unwrap();
    assert_eq!(sign_submission(&context, get("key")).unwrap(), get("proof"));
    assert!(verify_submission(&context, get("proof"), get("key")).unwrap());
    let mut other = context.clone();
    other.customer_id = "124".into();
    assert!(!verify_submission(&other, get("proof"), get("key")).unwrap());
    let cipher = MailCipher::new(get("key")).unwrap();
    assert_eq!(
        cipher
            .open::<String>(get("sealed"), get("purpose"))
            .unwrap(),
        get("value")
    );
    assert!(
        cipher
            .open::<String>(get("sealed"), "payload:124:other")
            .is_err()
    );
    let sealed = cipher.seal(&json!({"text":"ä"}), "purpose").unwrap();
    assert_eq!(
        cipher.open::<Value>(&sealed, "purpose").unwrap(),
        json!({"text":"ä"})
    );
    assert!(MailCipher::new("not a base64 key").is_err());
}

#[test]
fn sanitation_preserves_mime_bytes_and_removes_forged_authority_and_bcc() {
    let body = b"Private body\r\nBcc: body text\r\n\xff";
    let mut raw = b"From: Fake <bad@example.test>\r\nBcc: hidden@example.test\r\nX-Webdock-Submission: forged\r\n folded-secret\r\nDKIM-Signature: forged\r\nAuthentication-Results: forged\r\nSubject: Test\r\n\r\n".to_vec();
    raw.extend_from_slice(body);
    let sender = Sender {
        email: "SUPPORT@bücher.example".into(),
        name: Some("Änne König".into()),
    };
    let result = prepare_message(
        &raw,
        &sender,
        &["Alice@EXAMPLE.test".into(), "alice@example.test".into()],
    )
    .unwrap();
    assert_eq!(result.from, "support@xn--bcher-kva.example");
    assert_eq!(result.to, ["alice@example.test"]);
    let split = result
        .raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap();
    let headers = String::from_utf8_lossy(&result.raw[..split]);
    for forbidden in [
        "Bcc:",
        "X-Webdock",
        "DKIM",
        "Authentication",
        "forged",
        "Fake",
    ] {
        assert!(!headers.contains(forbidden));
    }
    assert_eq!(&result.raw[split + 4..], body);
    assert_eq!(result.hash.len(), 64);
    assert!(message_digest(&result.raw).is_err()); // Proof contract requires valid UTF-8.
    assert!(stamp_message(&result.raw, "123\r\nInjected: yes", &"x".repeat(43)).is_err());
    assert!(
        prepare_message(
            b" bad continuation\r\n\r\nBody",
            &sender,
            &["a@example.test".into()]
        )
        .is_err()
    );
    assert!(envelope_address("bad..address@example.test").is_err());
    assert!(envelope_address("sender@example.test\r\nBcc:evil@example.test").is_err());
    let bare = prepare_message(
        b"From: old@example.test\r\n\r\nExact body",
        &Sender {
            email: "a@example.test".into(),
            name: None,
        },
        &["b@example.test".into()],
    )
    .unwrap();
    assert_eq!(bare.raw, b"From: <a@example.test>\r\n\r\nExact body");
}

#[test]
fn request_compiler_keeps_bcc_private_and_refuses_external_attachments() {
    let input = json!({"from":"Sender <sender@example.test>","to":["A@EXAMPLE.TEST"],"cc":["a@example.test"],"bcc":["hidden@example.test"],"subject":"Hello","text":"Body","html":"<p>Body</p>","attachments":[{"filename":"hello.txt","content":"aGk="}]});
    let request = SendRequest::parse(input.clone(), chrono::Utc::now()).unwrap();
    assert_eq!(request.to, ["a@example.test"]);
    assert!(request.cc.is_empty());
    let message = request.compile().unwrap();
    let raw = String::from_utf8(message.raw).unwrap();
    assert!(raw.contains("hello.txt"));
    assert!(!raw.contains("Bcc:") && !raw.contains("hidden@example.test"));
    assert_eq!(message.bcc, ["hidden@example.test"]);
    for attachment in [
        json!({"path":"https://example.test/private"}),
        json!({"filename":"x","content":"!!!!"}),
        json!({"filename":"../x","content":"aGk="}),
    ] {
        let mut invalid = input.clone();
        invalid["attachments"] = json!([attachment]);
        assert!(SendRequest::parse(invalid, chrono::Utc::now()).is_err());
    }
    let stored = serde_json::to_value(&request).unwrap();
    assert_eq!(
        SendRequest::parse_stored(stored, chrono::Utc::now())
            .unwrap()
            .to,
        request.to
    );
}
