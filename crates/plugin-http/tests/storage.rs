use std::{collections::BTreeMap, net::IpAddr};
use webdock_plugin_http::storage::{StorageBinding, bind_storage, bind_storage_loopback_test};
fn binding() -> StorageBinding {
    StorageBinding {
        origin: "https://storage.invalid".into(),
        addresses: vec![IpAddr::from([127, 0, 0, 1])],
        ca: None,
        path_prefix: "/private/".into(),
    }
}
#[test]
fn signed_storage_lane_cannot_weaken_named_http_or_escape_operator_origin() {
    let b = binding();
    assert!(
        bind_storage(
            "GET",
            "https://storage.invalid/private/key?X-Amz-Signature=private",
            BTreeMap::new(),
            b.clone()
        )
        .is_ok()
    );
    for url in [
        "https://foreign.invalid/private/key",
        "https://storage.invalid/other/key",
        "https://user:password@storage.invalid/private/key",
        "http://storage.invalid/private/key",
        "https://storage.invalid/private/key#frag",
    ] {
        assert!(bind_storage("GET", url, BTreeMap::new(), b.clone()).is_err());
    }
    for key in [
        "Cookie",
        "Authorization",
        "Host",
        "Content-Length",
        "Proxy-Authorization",
        "Range",
        "x-amz-copy-source",
    ] {
        assert!(
            bind_storage(
                "PUT",
                "https://storage.invalid/private/key",
                BTreeMap::from([(key.into(), "secret".into())]),
                b.clone()
            )
            .is_err()
        );
    }
    assert!(
        bind_storage(
            "DELETE",
            "https://storage.invalid/private/key",
            BTreeMap::new(),
            b
        )
        .is_err()
    );
}
#[test]
fn insecure_transport_is_explicit_and_loopback_only() {
    let mut b = binding();
    b.origin = "http://127.0.0.1:9000".into();
    assert!(
        bind_storage_loopback_test(
            "GET",
            "http://127.0.0.1:9000/private/key",
            BTreeMap::new(),
            b.clone()
        )
        .is_ok()
    );
    assert!(
        bind_storage(
            "GET",
            "http://127.0.0.1:9000/private/key",
            BTreeMap::new(),
            b
        )
        .is_err()
    );
    let mut b = binding();
    b.origin = "http://169.254.169.254".into();
    b.addresses = vec!["169.254.169.254".parse().unwrap()];
    assert!(
        bind_storage_loopback_test(
            "GET",
            "http://169.254.169.254/private/key",
            BTreeMap::new(),
            b
        )
        .is_err()
    );
}

#[test]
fn binary_targets_require_objects_and_reject_administrative_or_ambiguous_queries() {
    for suffix in [
        "",
        "?policy",
        "?acl",
        "?lifecycle",
        "?versioning",
        "?uploads",
        "?delete",
        "?notification",
        "key?policy",
        "key?acl",
        "key?lifecycle",
        "key?uploadId=x",
        "key?partNumber=1",
        "key?versionId=a&versionId=b",
        "key?VersionId=a",
        "key?%76ersionId=a",
        "key?X-Amz-Signature=a&X-Amz-Signature=b",
        "key?x-amz-signature=a",
        "key?x-http-method-override=DELETE",
    ] {
        assert!(
            bind_storage(
                "PUT",
                &format!("https://storage.invalid/private/{suffix}"),
                BTreeMap::new(),
                binding()
            )
            .is_err(),
            "administrative/ambiguous target accepted"
        );
    }
    assert!(bind_storage("GET", "https://storage.invalid/private/key?versionId=null&X-Amz-SignedHeaders=host%3Bif-match&x-id=GetObject", BTreeMap::from([("if-match".into(), "\"abc-2\"".into())]), binding()).is_ok());
    assert!(bind_storage("PUT", "https://storage.invalid/private/key?X-Amz-SignedHeaders=host%3Bif-none-match%3Bx-amz-checksum-sha256&x-amz-sdk-checksum-algorithm=SHA256&x-id=PutObject", BTreeMap::from([("if-none-match".into(), "*".into()), ("x-amz-checksum-sha256".into(), "YQ==".into())]), binding()).is_ok());
}

#[test]
fn root_binding_still_requires_bucket_and_nonempty_object_key() {
    let mut b = binding();
    b.path_prefix = "/".into();
    for path in ["/", "/bucket", "/bucket/", "/bucket/?policy"] {
        assert!(
            bind_storage(
                "PUT",
                &format!("https://storage.invalid{path}"),
                BTreeMap::new(),
                b.clone()
            )
            .is_err()
        );
    }
    assert!(
        bind_storage(
            "PUT",
            "https://storage.invalid/bucket/key",
            BTreeMap::new(),
            b
        )
        .is_ok()
    );
}
