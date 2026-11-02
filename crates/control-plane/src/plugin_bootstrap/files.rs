use super::{Result, SetupError as Error};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
pub(super) fn directory(path: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(path).map_err(|_| Error::Output)?;
    if !path.is_absolute()
        || !meta.is_dir()
        || meta.uid() != rustix::process::geteuid().as_raw()
        || meta.mode() & 0o7777 != 0o700
        || std::fs::canonicalize(path).map_err(|_| Error::Output)? != path
    {
        return Err(Error::Output);
    }
    Ok(())
}
pub(super) fn read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    if !path.is_absolute() {
        return Err(Error::Input);
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)
        .map_err(|_| Error::Input)?;
    let meta = file.metadata().map_err(|_| Error::Input)?;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.uid() != rustix::process::geteuid().as_raw()
        || meta.mode() & 0o077 != 0
        || meta.len() > limit
    {
        return Err(Error::Input);
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Input)?;
    if bytes.len() as u64 > limit {
        return Err(Error::Input);
    }
    Ok(bytes)
}
pub(super) fn parent(path: &Path) -> Result<(&Path, &std::ffi::OsStr)> {
    let parent = path.parent().ok_or(Error::Output)?;
    directory(parent)?;
    Ok((parent, path.file_name().ok_or(Error::Output)?))
}
pub(super) fn write_exclusive(path: &Path, bytes: &[u8]) -> Result<()> {
    let (parent, _) = parent(path)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)
        .map_err(|_| Error::Output)?;
    file.write_all(bytes).map_err(|_| Error::Output)?;
    file.sync_all().map_err(|_| Error::Output)?;
    File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|_| Error::Output)
}
pub(super) fn publish(destination: &Path, files: &BTreeMap<String, Vec<u8>>) -> Result<()> {
    let (parent, name) = parent(destination)?;
    let stage_name = format!(
        ".webdock-plugin-bootstrap-{}",
        uuid::Uuid::new_v4().simple()
    );
    let stage = parent.join(&stage_name);
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&stage)
        .map_err(|_| Error::Output)?;
    struct Stage(PathBuf);
    impl Drop for Stage {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _stage = Stage(stage.clone());
    for (name, bytes) in files {
        write_exclusive(&stage.join(name), bytes)?;
    }
    File::open(&stage)
        .and_then(|f| f.sync_all())
        .map_err(|_| Error::Output)?;
    let parent_file = File::open(parent).map_err(|_| Error::Output)?;
    rustix::fs::renameat_with(
        &parent_file,
        &stage_name,
        &parent_file,
        name,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(|_| Error::Output)?;
    parent_file.sync_all().map_err(|_| Error::Output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn atomic_private_directory_publish_refuses_replacement_links_and_partial_files() {
        let parent = tempfile::tempdir().unwrap();
        std::fs::set_permissions(parent.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let destination = parent.path().join("kernel");
        let files = BTreeMap::from([("private.key".into(), b"retained-owned-key".to_vec())]);
        publish(&destination, &files).unwrap();
        assert_eq!(destination.metadata().unwrap().mode() & 0o777, 0o700);
        assert_eq!(
            destination.join("private.key").metadata().unwrap().mode() & 0o777,
            0o600
        );
        assert!(publish(&destination, &files).is_err());
        assert_eq!(
            read(&destination.join("private.key"), 100).unwrap(),
            b"retained-owned-key"
        );
        let link = parent.path().join("linked-key");
        std::os::unix::fs::symlink(destination.join("private.key"), &link).unwrap();
        assert!(read(&link, 100).is_err());
        std::fs::remove_file(&link).unwrap();
        std::fs::hard_link(destination.join("private.key"), &link).unwrap();
        assert!(read(&link, 100).is_err());
        assert!(write_exclusive(&link, b"replacement").is_err());
        let linked_directory = parent.path().join("linked-dir");
        std::os::unix::fs::symlink(&destination, &linked_directory).unwrap();
        assert!(directory(&linked_directory).is_err());
        assert!(publish(&linked_directory, &files).is_err());
    }
}
