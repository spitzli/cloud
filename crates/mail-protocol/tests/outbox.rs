use serde_json::json;
use webdock_mail_protocol::outbox::{PayloadFormat, PayloadLease, Preparation, Receipt, SendLease};

#[test]
fn native_outbox_dtos_preserve_order_names_and_strict_receipt_shapes() {
    let lease = SendLease {
        id: "789".into(),
        customer_id: "12".into(),
        source_id: "key:34".into(),
        payload_ref: "12345678-1234-4123-8123-123456789012".into(),
        payload_bytes: 2,
        request_hash: "a".repeat(64),
        payload_format: PayloadFormat::Request,
        source_context: json!({}),
        route: json!({"kind":"direct"}),
        envelope: json!({}),
        lease_token: "23456789-1234-4123-8123-123456789012".into(),
    };
    assert_eq!(
        serde_json::to_string(&lease).unwrap(),
        concat!(
            "{\"id\":\"789\",\"customerID\":\"12\",\"sourceID\":\"key:34\",",
            "\"payloadRef\":\"12345678-1234-4123-8123-123456789012\",\"payloadBytes\":2,",
            "\"requestHash\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",",
            "\"payloadFormat\":\"request\",\"sourceContext\":{},\"route\":{\"kind\":\"direct\"},\"envelope\":{},",
            "\"leaseToken\":\"23456789-1234-4123-8123-123456789012\"}"
        )
    );
    let mut unexpected = serde_json::to_value(&lease).unwrap();
    unexpected["caller"] = json!("forged");
    assert!(serde_json::from_value::<SendLease>(unexpected).is_err());
    let payload = PayloadLease {
        id: "789".into(),
        customer_id: "12".into(),
        payload_ref: lease.payload_ref,
        upload_token: lease.lease_token,
    };
    assert_eq!(
        serde_json::to_string(&payload).unwrap(),
        "{\"id\":\"789\",\"customerID\":\"12\",\"payloadRef\":\"12345678-1234-4123-8123-123456789012\",\"uploadToken\":\"23456789-1234-4123-8123-123456789012\"}"
    );
    let preparation = Preparation {
        metadata_sha256: "meta".into(),
        mime_sha256: "mime".into(),
        mime_bytes: 2,
    };
    assert_eq!(
        serde_json::to_string(&preparation).unwrap(),
        "{\"metadataSha256\":\"meta\",\"mimeSha256\":\"mime\",\"mimeBytes\":2}"
    );
    assert_eq!(
        serde_json::to_string(&Receipt::Accepted {
            provider_id: "receipt_1".into()
        })
        .unwrap(),
        "{\"state\":\"accepted\",\"providerID\":\"receipt_1\"}"
    );
    for state in ["rejected", "unknown"] {
        let value = json!({"state":state});
        let receipt: Receipt = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(receipt).unwrap(), value);
        assert!(
            serde_json::from_value::<Receipt>(json!({"state":state,"providerID":"untrusted"}))
                .is_err()
        );
    }
}
