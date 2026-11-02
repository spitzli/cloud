use std::collections::BTreeMap;
use webdock_identity::{CookiePolicy, SecretKeys, hash_password, verify_password};
const SECRET: &str = "fixture-secret-with-at-least-32-bytes";
const HASH: &str = "000102030405060708090a0b0c0d0e0f:c7877280641a617ec087f9365d3ad7facccc9681e2af94d0a148b29a67b413ab0e08c7f08f9bd6232c369f5bbd3eec03acc2a170c24fd179aecc67c282756d12";

#[test]
fn password_matches_node_scrypt_with_nfkc_and_ascii_hex_salt() {
    assert!(verify_password("CaféÅ-password-123", HASH).unwrap());
    assert!(verify_password("Cafe\u{301}Å-password-123", HASH).unwrap());
    assert!(!verify_password("wrong-password", HASH).unwrap());
    for malformed in ["", "ab:cd", &format!("{HASH}:extra")] {
        assert!(verify_password("password", malformed).is_err());
    }
    let generated = hash_password("new-password-123").unwrap();
    assert!(verify_password("new-password-123", &generated).unwrap());
}

#[test]
fn signed_cookie_matches_node_and_rejects_duplicates_and_tampering() {
    let policy = CookiePolicy::new(true, SECRET).unwrap();
    let value = "test-session-token.NrQHC3bG3bO3e2sWpuTMDveCOitoPFy8QOxe2OP40rY%3D";
    assert_eq!(policy.sign("test-session-token").unwrap(), value);
    let cookie = format!("{}={value}", policy.name());
    assert_eq!(
        policy.read(&cookie).unwrap().as_deref(),
        Some("test-session-token")
    );
    assert!(
        policy
            .read(&format!("{cookie}; {cookie}"))
            .unwrap()
            .is_none()
    );
    assert!(
        policy
            .read(&cookie.replace("test-session-token", "attacker"))
            .unwrap()
            .is_none()
    );
    assert!(
        policy
            .read(&format!("{}=x.%GG", policy.name()))
            .unwrap()
            .is_none()
    );
    let header = policy.header("test-session-token", 28800).unwrap();
    assert!(header.contains("; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age=28800"));
    assert!(!header.contains("Domain="));
}

#[test]
fn encrypted_material_matches_javascript_and_preserves_key_versions() {
    let encrypted = "070707070707070707070707070707070707070707070707c234ca2d492171a95225b6fd8ed4bb5c01aa2ff753470cfeb139a585f6aeef8ddab0d7fa327e62b08cca54a8fc";
    let keys = SecretKeys::new(
        BTreeMap::from([(4, SECRET.to_owned())]),
        4,
        Some(SECRET.to_owned()),
    )
    .unwrap();
    let plain = "[\"abcde-fghij\",\"12345-67890\"]";
    assert_eq!(keys.decrypt(encrypted).unwrap(), plain);
    assert_eq!(keys.decrypt(&format!("$ba$4${encrypted}")).unwrap(), plain);
    assert!(keys.decrypt(&format!("$ba$5${encrypted}")).is_err());
    assert!(keys.decrypt(&format!("{encrypted}00")).is_err());
    let fresh = keys.encrypt(plain).unwrap();
    assert!(fresh.starts_with("$ba$4$"));
    assert_eq!(keys.decrypt(&fresh).unwrap(), plain);
}
