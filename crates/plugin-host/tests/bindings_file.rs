use std::os::unix::fs::PermissionsExt;
use webdock_plugin_host::{FileBindings, RuntimeBindings};
#[tokio::test]
async fn transport_enrollment_file_must_be_private_and_not_a_symlink() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bindings.json");
    std::fs::write(&path, b"{\"runtimes\":{}}").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        FileBindings::new(path.clone())
            .runtime("provider")
            .await
            .unwrap()
            .is_none()
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
    assert!(
        FileBindings::new(path.clone())
            .runtime("provider")
            .await
            .is_err()
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let link = directory.path().join("link.json");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert!(FileBindings::new(link).runtime("provider").await.is_err());
}
