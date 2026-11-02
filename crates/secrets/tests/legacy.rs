use serde_json::Value;
use webdock_secrets::SecretCipher;
#[test]
fn retained_node_ciphertext_requires_the_original_key_and_exact_purpose() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../hosting/tests/node-environment.json")).unwrap();
    let cipher = SecretCipher::new(fixture["secret"].as_str().unwrap()).unwrap();
    let value: Value = cipher
        .open(fixture["sealed"].as_str().unwrap(), "hosting-env:123")
        .unwrap();
    assert_eq!(value, fixture["values"]);
    assert!(
        cipher
            .open::<Value>(fixture["sealed"].as_str().unwrap(), "hosting-env:456")
            .is_err()
    );
    assert!(
        SecretCipher::new("a-different-private-fixture-key-of-32-bytes")
            .unwrap()
            .open::<Value>(fixture["sealed"].as_str().unwrap(), "hosting-env:123")
            .is_err()
    );
    let sealed = cipher
        .seal(
            &serde_json::json!({"password":"synthetic-fixture-password"}),
            "database:123:1",
        )
        .unwrap();
    assert_eq!(
        cipher.open::<Value>(&sealed, "database:123:1").unwrap()["password"],
        "synthetic-fixture-password"
    );
    assert!(cipher.open::<Value>(&sealed, "database:123:2").is_err());
    assert!(cipher.open::<Value>(&"x".repeat(32769), "purpose").is_err());
    assert!(cipher.seal(&"x".repeat(32769), "purpose").is_err());
}
