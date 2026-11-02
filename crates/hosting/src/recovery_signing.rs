//! Independent operator-installed admitted-set issuer. No signing key is an API field.
use crate::{Result, error};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use webdock_agent_protocol::recovery::custody::{AdmittedRecoverySet, SignedDocument};
pub struct RecoverySigner {
    key: PathBuf,
    directory: PathBuf,
}
fn private(path: &Path, directory: bool) -> Result<()> {
    let m = std::fs::symlink_metadata(path)
        .map_err(|_| error(503, "Recovery custody is unavailable."))?;
    if !path.is_absolute()
        || m.file_type().is_symlink()
        || m.uid() != rustix::process::geteuid().as_raw()
        || m.mode() & 0o077 != 0
        || (if directory {
            !m.is_dir()
        } else {
            !m.is_file() || m.nlink() != 1 || m.len() > 262144
        })
    {
        return Err(error(
            503,
            "Private independent recovery custody is required.",
        ));
    }
    Ok(())
}
impl RecoverySigner {
    pub fn new(key: PathBuf, directory: PathBuf) -> Result<Self> {
        private(&key, false)?;
        private(&directory, true)?;
        Ok(Self { key, directory })
    }
    pub(crate) fn sign(&self, set: &AdmittedRecoverySet) -> Result<SignedDocument> {
        self.sign_document(set, "webdock-recovery-set-v1")
    }
    pub(crate) fn sign_reservation(&self, grant: &serde_json::Value) -> Result<SignedDocument> {
        self.sign_document(grant, "webdock-recovery-reservation-v1")
    }
    fn sign_document(
        &self,
        value: &impl serde::Serialize,
        namespace: &str,
    ) -> Result<SignedDocument> {
        private(&self.key, false)?;
        private(&self.directory, true)?;
        let bytes = serde_json::to_vec(value)
            .map_err(|_| error(503, "Recovery admission encoding failed."))?;
        if bytes.len() > 131072 {
            return Err(error(409, "Recovery admission exceeds the custody bound."));
        }
        let directory = tempfile::Builder::new()
            .prefix("recovery-admission-")
            .tempdir_in(&self.directory)
            .map_err(|_| error(503, "Recovery custody is unavailable."))?;
        let path = directory.path().join("set");
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .map_err(|_| error(503, "Recovery custody is unavailable."))?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| error(503, "Recovery custody is unavailable."))?;
        let mut process = Command::new("/usr/bin/ssh-keygen")
            .args(["-Y", "sign", "-q", "-f"])
            .arg(&self.key)
            .args(["-n", namespace])
            .arg(&path)
            .env("SSH_ASKPASS_REQUIRE", "never")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| error(503, "Recovery signer is unavailable."))?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = process
                .try_wait()
                .map_err(|_| error(503, "Recovery signer is unavailable."))?
            {
                if !status.success() {
                    return Err(error(503, "Recovery signer rejected admission."));
                }
                break;
            }
            if Instant::now() >= deadline {
                let _ = process.kill();
                let _ = process.wait();
                return Err(error(503, "Recovery signer deadline exceeded."));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let signature = path.with_extension("sig");
        std::fs::set_permissions(&signature, std::fs::Permissions::from_mode(0o600))
            .map_err(|_| error(503, "Recovery custody is unavailable."))?;
        private(&signature, false)?;
        let signature =
            std::fs::read(signature).map_err(|_| error(503, "Recovery custody is unavailable."))?;
        Ok(SignedDocument {
            payload: STANDARD.encode(bytes),
            signature: STANDARD.encode(signature),
        })
    }
}
