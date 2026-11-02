//! Dedicated retained volume. All filesystem operations stay relative to an owned directory FD.
//! Call through a bounded blocking worker; these durability operations intentionally fsync.
use crate::{Error, Result, message::MAX_MESSAGE, proof::valid_id, secrets::MailCipher};
use base64::{Engine, engine::general_purpose::STANDARD};
use rustix::{
    fs::{self, AtFlags, Mode, OFlags},
    io::Errno,
    process::geteuid,
};
use std::{
    fs::File,
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::Path,
    sync::Arc,
};
use uuid::Uuid;
const SEALED_LIMIT: usize = 48 * 1024 * 1024;
const MARKER: &str = ".mail-key-check";
#[derive(Clone)]
pub struct PrivatePayloadStore {
    directory: Arc<File>,
    cipher: Arc<MailCipher>,
}
pub struct SavedPayload {
    pub reference: String,
    pub bytes: usize,
}
impl PrivatePayloadStore {
    pub fn open(directory: &Path, key: &str) -> Result<Self> {
        if !directory.is_absolute() {
            return Err(Error::Storage);
        }
        let cipher = MailCipher::new(key)?;
        let file = File::from(
            fs::open(
                directory,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| Error::Storage)?,
        );
        let meta = file.metadata().map_err(|_| Error::Storage)?;
        if !meta.is_dir() || meta.uid() != geteuid().as_raw() || meta.mode() & 0o077 != 0 {
            return Err(Error::Storage);
        }
        let store = Self {
            directory: Arc::new(file),
            cipher: Arc::new(cipher),
        };
        match fs::openat(
            &store.directory,
            MARKER,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(_) => {}
            Err(Errno::NOENT) => {
                let sealed = store
                    .cipher
                    .seal(&"webdock-private-payload-v1", "payload-store")?;
                match store.install(MARKER, sealed.as_bytes()) {
                    Ok(()) | Err(Errno::EXIST) => {}
                    Err(_) => return Err(Error::Storage),
                }
            }
            Err(_) => return Err(Error::Storage),
        }
        let sealed = store.read(MARKER, 4096)?;
        let value: String = store.cipher.open(
            std::str::from_utf8(&sealed).map_err(|_| Error::Secret)?,
            "payload-store",
        )?;
        if value != "webdock-private-payload-v1" {
            return Err(Error::Secret);
        }
        Ok(store)
    }
    fn install(&self, name: &str, bytes: &[u8]) -> std::result::Result<(), Errno> {
        let temp = format!(".mail-write-{}", Uuid::new_v4());
        let fd = fs::openat(
            &self.directory,
            &temp,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?;
        let mut file = File::from(fd);
        let result = (|| {
            file.write_all(bytes)
                .map_err(|e| Errno::from_raw_os_error(e.raw_os_error().unwrap_or(5)))?;
            fs::fsync(&file)?;
            fs::linkat(
                &self.directory,
                &temp,
                &self.directory,
                name,
                AtFlags::empty(),
            )?;
            fs::fsync(&self.directory)
        })();
        let cleanup = fs::unlinkat(&self.directory, &temp, AtFlags::empty());
        fs::fsync(&self.directory)?;
        result.and(cleanup)
    }
    fn read(&self, name: &str, limit: usize) -> Result<Vec<u8>> {
        let file = File::from(
            fs::openat(
                &self.directory,
                name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| Error::Storage)?,
        );
        let meta = file.metadata().map_err(|_| Error::Storage)?;
        if !meta.is_file()
            || meta.uid() != geteuid().as_raw()
            || meta.mode() & 0o077 != 0
            || meta.len() > limit as u64
        {
            return Err(Error::Storage);
        }
        let mut value = Vec::new();
        file.take((limit + 1) as u64)
            .read_to_end(&mut value)
            .map_err(|_| Error::Storage)?;
        if value.len() > limit {
            return Err(Error::Storage);
        }
        Ok(value)
    }
    pub fn put(
        &self,
        customer: &str,
        body: &[u8],
        reference: Option<&str>,
    ) -> Result<SavedPayload> {
        if body.is_empty() || body.len() > MAX_MESSAGE {
            return Err(Error::Invalid);
        }
        let reference = reference
            .map(str::to_owned)
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let purpose = purpose(customer, &reference)?;
        let sealed = self.cipher.seal(&STANDARD.encode(body), &purpose)?;
        let space = fs::fstatvfs(&self.directory).map_err(|_| Error::Storage)?;
        if u128::from(space.f_bavail) * u128::from(space.f_frsize)
            < (sealed.len() as u128) * 2 + 104_857_600
        {
            return Err(Error::Storage);
        }
        self.install(&reference, sealed.as_bytes())
            .map_err(|_| Error::Storage)?;
        Ok(SavedPayload {
            reference,
            bytes: body.len(),
        })
    }
    pub fn get(&self, customer: &str, reference: &str) -> Result<Vec<u8>> {
        let purpose = purpose(customer, reference)?;
        let sealed = self.read(reference, SEALED_LIMIT)?;
        let encoded: String = self.cipher.open(
            std::str::from_utf8(&sealed).map_err(|_| Error::Secret)?,
            &purpose,
        )?;
        let value = STANDARD.decode(&encoded).map_err(|_| Error::Secret)?;
        if value.is_empty() || value.len() > MAX_MESSAGE || STANDARD.encode(&value) != encoded {
            return Err(Error::Secret);
        }
        Ok(value)
    }
    /// Internal cleanup only; admission must bind an authorized job before calling.
    pub fn remove(&self, reference: &str) -> Result<()> {
        valid_reference(reference)?;
        match fs::unlinkat(&self.directory, reference, AtFlags::empty()) {
            Ok(()) | Err(Errno::NOENT) => {}
            Err(_) => return Err(Error::Storage),
        }
        fs::fsync(&self.directory).map_err(|_| Error::Storage)
    }
}
fn valid_reference(reference: &str) -> Result<()> {
    let id = Uuid::parse_str(reference).map_err(|_| Error::Invalid)?;
    if id.get_version_num() != 4
        || id.get_variant() != uuid::Variant::RFC4122
        || id.to_string() != reference
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
fn purpose(customer: &str, reference: &str) -> Result<String> {
    if !valid_id(customer) {
        return Err(Error::Invalid);
    }
    valid_reference(reference)?;
    Ok(format!("payload:{customer}:{reference}"))
}

static BLOCKING: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
async fn blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    let permit = BLOCKING.acquire().await.map_err(|_| Error::Storage)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        operation()
    })
    .await
    .map_err(|_| Error::Storage)?
}
#[async_trait::async_trait]
impl crate::PrivatePayloadStore for PrivatePayloadStore {
    async fn put(&self, customer: &str, reference: &str, bytes: &[u8]) -> Result<SavedPayload> {
        let (store, customer, reference, bytes) = (
            self.clone(),
            customer.to_owned(),
            reference.to_owned(),
            bytes.to_owned(),
        );
        blocking(move || store.put(&customer, &bytes, Some(&reference))).await
    }
    async fn get(&self, customer: &str, reference: &str) -> Result<Vec<u8>> {
        let (store, customer, reference) =
            (self.clone(), customer.to_owned(), reference.to_owned());
        blocking(move || store.get(&customer, &reference)).await
    }
    async fn remove(&self, customer: &str, reference: &str) -> Result<()> {
        let (store, customer, reference) =
            (self.clone(), customer.to_owned(), reference.to_owned());
        blocking(move || {
            purpose(&customer, &reference)?;
            match fs::statat(
                &store.directory,
                reference.as_str(),
                AtFlags::SYMLINK_NOFOLLOW,
            ) {
                Err(Errno::NOENT) => return store.remove(&reference),
                Err(_) => return Err(Error::Storage),
                Ok(_) => {
                    store.get(&customer, &reference)?;
                }
            }
            store.remove(&reference)
        })
        .await
    }
}
