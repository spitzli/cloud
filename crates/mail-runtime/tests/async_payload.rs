use std::{os::unix::fs::PermissionsExt, sync::Arc};
use webdock_mail_runtime::{FilesystemPayloadStore, PrivatePayloadStore};

#[tokio::test]
async fn scoped_async_custody_is_immutable_and_cannot_remove_another_customer() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let filesystem = Arc::new(
        FilesystemPayloadStore::open(
            directory.path(),
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        )
        .unwrap(),
    );
    let store: &dyn PrivatePayloadStore = filesystem.as_ref();
    let reference = uuid::Uuid::new_v4().to_string();
    let saved = store
        .put("12", &reference, b"private payload")
        .await
        .unwrap();
    assert_eq!(saved.reference, reference);
    assert_eq!(saved.bytes, 15);
    assert!(store.put("12", &reference, b"replacement").await.is_err());
    assert!(store.remove("13", &reference).await.is_err());
    assert_eq!(
        store.get("12", &reference).await.unwrap(),
        b"private payload"
    );
    store.remove("12", &reference).await.unwrap();
    store.remove("12", &reference).await.unwrap();
    assert!(store.get("12", &reference).await.is_err());
}
