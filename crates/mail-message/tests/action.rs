use serde_json::json;
use webdock_mail_message::compile_action;

#[test]
fn action_compilation_preserves_bounded_input_and_canonical_native_bytes() {
    let result = compile_action(json!({"resource_id":"owned-resource","from":" info ","from_name":" Team ","to":" a@example.test, ,b@example.test ","subject":" Subject ","text":"Body"}), "example.test").unwrap();
    assert_eq!(result.payload, br#"{"from":{"email":"info@example.test","name":"Team"},"to":["a@example.test","b@example.test"],"subject":"Subject","text":"Body","cc":[],"bcc":[]}"#);
    assert_eq!(
        result.envelope,
        json!({"from":"info@example.test","to":["a@example.test","b@example.test"]})
    );
    assert_eq!(
        result.request.compile().unwrap().sender.email,
        "info@example.test"
    );
}

#[test]
fn action_compilation_rejects_unbounded_or_caller_authority() {
    let input = json!({"resource_id":"owned-resource","from":"info","to":"a@example.test","subject":"Subject","text":"Body"});
    for (field, value) in [
        ("from", json!("a..b")),
        ("from_name", json!("x".repeat(81))),
        (
            "to",
            json!(
                (0..11)
                    .map(|_| "a@example.test")
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        ),
        ("subject", json!("x".repeat(201))),
        ("text", json!("x".repeat(30_001))),
        ("customerID", json!("123")),
    ] {
        let mut changed = input.clone();
        changed[field] = value;
        assert!(compile_action(changed, "example.test").is_err(), "{field}");
    }
}

#[test]
fn action_preparation_keeps_80_unicode_chars_while_native_keeps_80_utf16_units() {
    use webdock_mail_message::{SendRequest, prepare_action_message};
    for count in [41, 80] {
        let name = "😀".repeat(count);
        let compiled = compile_action(json!({"resource_id":"owned-resource","from":"info","from_name":name,"to":"a@example.test","subject":"Subject","text":"Body"}), "example.test").unwrap();
        let stored = serde_json::from_slice(&compiled.payload).unwrap();
        let request = SendRequest::parse_stored_action(stored).unwrap();
        let message = request.compile_action().unwrap();
        let prepared = prepare_action_message(&message.raw, &message.sender, &message.to).unwrap();
        assert_eq!(message.sender.name.as_deref(), Some(name.as_str()));
        assert_eq!(prepared.from, "info@example.test");
        assert_eq!(prepared.to, ["a@example.test"]);
        assert!(SendRequest::parse(json!({"from":format!("{name} <info@example.test>"),"to":"a@example.test","subject":"Subject","text":"Body"}), chrono::Utc::now()).is_err());
        assert!(request.compile().is_err());
    }
    let compiled = compile_action(json!({"resource_id":"owned-resource","from":"info","to":"a@example.test","subject":"Subject","text":"Body"}), "example.test").unwrap();
    let mut stored: serde_json::Value = serde_json::from_slice(&compiled.payload).unwrap();
    stored["from"]["name"] = json!("😀".repeat(81));
    assert!(SendRequest::parse_stored_action(stored).is_err());
}

#[test]
fn stored_action_profile_cannot_expand_the_plugin_input_contract() {
    use webdock_mail_message::SendRequest;
    let compiled = compile_action(json!({"resource_id":"owned-resource","from":"info","from_name":"Quoted <team>","to":"a@example.test","subject":"Subject","text":"Body"}), "example.test").unwrap();
    let stored: serde_json::Value = serde_json::from_slice(&compiled.payload).unwrap();
    let request = SendRequest::parse_stored_action(stored.clone()).unwrap();
    assert_eq!(
        request.compile_action().unwrap().sender.name.as_deref(),
        Some("Quoted <team>")
    );
    for (field, value) in [
        ("cc", json!(["cc@example.test"])),
        ("bcc", json!(["bcc@example.test"])),
        ("attachments", json!([])),
        ("html", json!("<p>Body</p>")),
        ("scheduled_at", json!("2026-10-25T00:00:00Z")),
        (
            "to",
            json!((0..11).map(|_| "a@example.test").collect::<Vec<_>>()),
        ),
        ("subject", json!("x".repeat(201))),
        ("text", json!("x".repeat(30_001))),
        ("actionProfile", json!(true)),
    ] {
        let mut invalid = stored.clone();
        invalid[field] = value;
        assert!(
            SendRequest::parse_stored_action(invalid).is_err(),
            "{field}"
        );
    }
    let native = SendRequest::parse(json!({"from":format!("{} <info@example.test>","😀".repeat(40)),"to":"a@example.test","subject":"Subject","text":"Body"}),chrono::Utc::now()).unwrap();
    assert!(native.compile().is_ok());
}
