//! Staging is a separate namespace/capability lane, never a retained release receipt.
use spitzli_object_storage::{Credentials, S3Config, StagingMethod, Store};
use std::time::Duration;
#[tokio::test]
async fn presigned_access_is_method_scope_and_expiry_bound_without_cloud_calls() {
    let store = Store::s3(
        S3Config {
            id: "staging".into(),
            endpoint: "https://objects.example.invalid".into(),
            region: "eu-test-1".into(),
            bucket: "owned-build-staging".into(),
            namespace: "isolated".into(),
        },
        Credentials::new("owned-test-key".into(), "owned-test-secret".into()).unwrap(),
    )
    .unwrap();
    let scope = "build-attempts/123/456/789/1";
    let get = store
        .presign_staging(
            scope,
            "source.tar",
            StagingMethod::Get,
            Duration::from_secs(60),
        )
        .await
        .unwrap();
    let put = store
        .presign_staging(
            scope,
            "source.tar",
            StagingMethod::Put,
            Duration::from_secs(60),
        )
        .await
        .unwrap();
    assert_eq!(
        get.path(),
        "/owned-build-staging/isolated/build-attempts/123/456/789/1/source.tar"
    );
    assert_ne!(get.query(), put.query());
    assert!(
        get.query_pairs()
            .any(|(k, v)| k.eq_ignore_ascii_case("x-amz-expires") && v == "60")
    );
    for bad in ["../other", "/other", "other?query", "other#fragment"] {
        assert!(
            store
                .presign_staging(
                    bad,
                    "source.tar",
                    StagingMethod::Get,
                    Duration::from_secs(60)
                )
                .await
                .is_err()
        );
        assert!(
            store
                .presign_staging(scope, bad, StagingMethod::Put, Duration::from_secs(60))
                .await
                .is_err()
        );
    }
    for seconds in [0, 1801] {
        assert!(
            store
                .presign_staging(
                    scope,
                    "source.tar",
                    StagingMethod::Get,
                    Duration::from_secs(seconds)
                )
                .await
                .is_err()
        );
    }
}
#[cfg(feature = "local-test")]
#[tokio::test]
async fn staging_retries_cannot_overwrite_different_source_and_reads_remain_bounded() {
    let store = Store::memory("staging", "isolated").unwrap();
    store
        .write_staging("build-attempts/1/2/3/1", "source.tar", b"exact source", 64)
        .await
        .unwrap();
    store
        .write_staging("build-attempts/1/2/3/1", "source.tar", b"exact source", 64)
        .await
        .unwrap();
    assert!(
        store
            .write_staging(
                "build-attempts/1/2/3/1",
                "source.tar",
                b"changed source",
                64
            )
            .await
            .is_err()
    );
    assert!(
        store
            .read_staging("build-attempts/1/2/3/1", "source.tar", 4)
            .await
            .is_err()
    );
    assert_eq!(
        store
            .read_staging("build-attempts/1/2/3/1", "source.tar", 64)
            .await
            .unwrap(),
        b"exact source"
    );
    store
        .write_staging("build-attempts/9/2/3/1", "retained.tar", b"other bytes", 64)
        .await
        .unwrap();
    store
        .delete_staging("build-attempts/1/2/3/1", "source.tar")
        .await
        .unwrap();
    store
        .delete_staging("build-attempts/1/2/3/1", "source.tar")
        .await
        .unwrap();
    assert_eq!(
        store
            .read_staging("build-attempts/9/2/3/1", "retained.tar", 64)
            .await
            .unwrap(),
        b"other bytes"
    );
    assert!(
        store
            .read_staging("build-attempts/9/2/3/1", "source.tar", 64)
            .await
            .is_err()
    );
}
#[test]
fn private_ca_input_must_contain_actual_valid_certificates() {
    for bytes in [
        b"".as_slice(),
        b"not a CA certificate",
        b"-----BEGIN CERTIFICATE-----\nYXNk\n-----END CERTIFICATE-----\n",
    ] {
        let config = S3Config {
            id: "staging".into(),
            endpoint: "https://objects.example.invalid".into(),
            region: "eu-test-1".into(),
            bucket: "owned-build-staging".into(),
            namespace: "isolated".into(),
        };
        assert!(
            Store::s3_with_ca(
                config,
                Credentials::new("owned-test-key".into(), "owned-test-secret".into()).unwrap(),
                bytes
            )
            .is_err()
        );
    }
}

#[tokio::test]
async fn build_source_capability_has_only_get_authority_and_validated_attempt_scope() {
    let store = Store::s3(
        S3Config {
            id: "staging".into(),
            endpoint: "https://objects.example.invalid".into(),
            region: "eu-test-1".into(),
            bucket: "owned-build-staging".into(),
            namespace: "isolated".into(),
        },
        Credentials::new("owned-test-key".into(), "owned-test-secret".into()).unwrap(),
    )
    .unwrap();
    let scope = "build-attempts/customer/project/build/1";
    let source = store
        .presign_build_source(scope, Duration::from_secs(60))
        .await
        .unwrap();
    let get = store
        .presign_staging(
            scope,
            "source.tar",
            StagingMethod::Get,
            Duration::from_secs(60),
        )
        .await
        .unwrap();
    assert_eq!(source.path(), get.path());
    assert!(
        source
            .query_pairs()
            .any(|(k, v)| k.eq_ignore_ascii_case("x-amz-expires") && v == "60")
    );
    for scope in [
        "build-attempts/customer/project/build/0",
        "build-attempts/customer/project/build/1/extra",
        "retained/customer/project/build/1",
        "build-attempts/customer/../build/1",
        "build-attempts/customer/project/build/not-generation",
    ] {
        assert!(
            store
                .presign_build_source(scope, Duration::from_secs(60))
                .await
                .is_err()
        );
    }
}
