//! LOCAL ONLY. Pinned official mc owns madmin encryption and signing. Never a production backend.
use crate::model::*;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    process::Stdio,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
const MC_SHA256: &str = "01f866e9c5f9b87c2b09116fa5d7c06695b106242d829a8bb32990c00312e891";
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MinioConfig {
    pub executable: PathBuf,
    pub config_dir: PathBuf,
}
pub struct Minio {
    executable: PathBuf,
    config_dir: PathBuf,
}
fn error<T>(_: T) -> ProviderError {
    ProviderError::Unknown
}
// The test image reads only the retained private runtime JSON; credentials never enter an image.
#[cfg(feature = "local-native-fixture")]
pub fn prepare_native_fixture(
    c: &MinioConfig,
    endpoint: &str,
    access: &str,
    secret: &str,
) -> Result<(), &'static str> {
    use std::os::unix::fs::DirBuilderExt;
    if c.executable != std::path::Path::new("/usr/local/bin/mc")
        || c.config_dir != std::path::Path::new("/tmp/webdock-minio")
    {
        return Err("Fixed native fixture mc paths required");
    }
    match std::fs::DirBuilder::new().mode(0o700).create(&c.config_dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = std::fs::symlink_metadata(&c.config_dir)
                .map_err(|_| "Private mc directory required")?;
            if !metadata.is_dir()
                || metadata.uid() != rustix::process::geteuid().as_raw()
                || metadata.mode() & 0o777 != 0o700
            {
                return Err("Private mc directory required");
            }
            // Minio::new verifies retained alias identity and private file ownership next.
            return Ok(());
        }
        Err(_) => return Err("Private mc directory required"),
    }
    let alias = serde_json::json!({"version":"10","aliases":{"local":{
        "url":endpoint,"accessKey":access,"secretKey":secret,"api":"S3v4","path":"on"
    }}});
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(c.config_dir.join("config.json"))
        .map_err(|_| "Private mc config unavailable")?;
    std::io::Write::write_all(&mut file, alias.to_string().as_bytes())
        .map_err(|_| "Private mc config unavailable")
}
impl Minio {
    pub async fn new(
        c: MinioConfig,
        endpoint: &str,
        access: &str,
        secret: &str,
    ) -> Result<Self, &'static str> {
        if !c.executable.is_absolute() {
            return Err("Absolute mc executable path required");
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&c.executable)
            .map_err(|_| "MinIO helper unavailable")?;
        let metadata = file.metadata().map_err(|_| "MinIO helper unavailable")?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o022 != 0
            || metadata.len() > 67108864
        {
            return Err("Untrusted mc executable");
        }
        let mut binary = Vec::new();
        std::io::Read::read_to_end(&mut std::io::Read::take(file, 67108865), &mut binary)
            .map_err(|_| "MinIO helper unavailable")?;
        if hex::encode(Sha256::digest(binary)) != MC_SHA256 {
            return Err("Pinned official mc executable required");
        }
        let directory = std::fs::symlink_metadata(&c.config_dir)
            .map_err(|_| "Private mc directory required")?;
        if !directory.is_dir() || directory.permissions().mode() & 0o077 != 0 {
            return Err("Private mc directory required");
        }
        let config = webdock_plugin_kit::read_file(&c.config_dir.join("config.json"), true)
            .map_err(|_| "Private mc config required")?;
        let config: Value = serde_json::from_slice(&config).map_err(|_| "Invalid mc config")?;
        let alias = &config["aliases"]["local"];
        if alias["url"] != endpoint || alias["accessKey"] != access || alias["secretKey"] != secret
        {
            return Err("mc binding differs from explicit local backend");
        }
        Ok(Self {
            executable: c.executable,
            config_dir: c.config_dir,
        })
    }
    async fn run(
        &self,
        args: &[&str],
        stdin: Option<String>,
    ) -> ProviderResult<(bool, Vec<Value>)> {
        let mut child = tokio::process::Command::new(&self.executable)
            .env_clear()
            .env(
                "HOME",
                std::env::var_os("HOME").ok_or(ProviderError::Unavailable)?,
            )
            .env("MC_CONFIG_DIR", &self.config_dir)
            .arg("--json")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(error)?;
        let mut input = child.stdin.take().ok_or(ProviderError::Unknown)?;
        if let Some(data) = stdin {
            input.write_all(data.as_bytes()).await.map_err(error)?;
        }
        drop(input);
        let mut bytes = Vec::new();
        child
            .stdout
            .take()
            .ok_or(ProviderError::Unknown)?
            .take(65537)
            .read_to_end(&mut bytes)
            .await
            .map_err(error)?;
        if bytes.len() > 65536 {
            return Err(ProviderError::Unknown);
        }
        let status = child.wait().await.map_err(error)?;
        let records = serde_json::Deserializer::from_slice(&bytes)
            .into_iter::<Value>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(error)?;
        if records.is_empty() && !status.success() {
            return Err(ProviderError::Unknown);
        }
        Ok((status.success(), records))
    }
    async fn command(&self, args: &[&str], stdin: Option<String>) -> ProviderResult<Vec<Value>> {
        let (success, records) = self.run(args, stdin).await?;
        if !success || records.iter().any(|v| v["status"] != "success") {
            return Err(ProviderError::Unknown);
        }
        Ok(records)
    }
    pub async fn exists(&self, b: &Binding) -> ProviderResult<bool> {
        let (success, records) = self
            .run(&["admin", "user", "info", "local", &b.user], None)
            .await?;
        if success {
            return Ok(true);
        }
        if records.iter().any(no_such_user) {
            Ok(false)
        } else {
            Err(ProviderError::Unknown)
        }
    }
    pub async fn provision(&self, b: &Binding, policy: &Value) -> ProviderResult<()> {
        if self.exists(b).await? {
            return Err(ProviderError::Collision);
        }
        // Neither the parent password nor admin credentials appear in argv or process output logs.
        self.command(
            &["admin", "user", "add", "local"],
            Some(format!(
                "{}\n{}\n",
                b.user,
                webdock_plugin_kit::seal::password()
            )),
        )
        .await?;
        let path = self
            .config_dir
            .join(format!("{}.policy.json", b.id.simple()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(error)?;
        std::io::Write::write_all(&mut file, policy.to_string().as_bytes()).map_err(error)?;
        self.command(
            &[
                "admin",
                "policy",
                "create",
                "local",
                &b.user,
                path.to_str().ok_or(ProviderError::Unknown)?,
            ],
            None,
        )
        .await?;
        self.command(
            &[
                "admin", "policy", "attach", "local", &b.user, "--user", &b.user,
            ],
            None,
        )
        .await?;
        Ok(())
    }
    pub async fn keys(&self, b: &Binding) -> ProviderResult<Vec<String>> {
        let values = self
            .command(&["admin", "user", "svcacct", "ls", "local", &b.user], None)
            .await?;
        let mut out = Vec::new();
        for v in values {
            if let Some(key) = v["accessKey"].as_str() {
                out.push(key.to_owned());
            }
            if let Some(accounts) = v["accounts"].as_array() {
                for a in accounts {
                    out.push(
                        a["accessKey"]
                            .as_str()
                            .ok_or(ProviderError::Unknown)?
                            .to_owned(),
                    );
                }
            }
        }
        Ok(out)
    }
    pub async fn create_key(&self, b: &Binding) -> ProviderResult<SecretKey> {
        let values = self
            .command(
                &[
                    "admin",
                    "user",
                    "svcacct",
                    "add",
                    "local",
                    &b.user,
                    "--name",
                    "webdock-app",
                ],
                None,
            )
            .await?;
        let v = values.first().ok_or(ProviderError::Unknown)?;
        Ok(SecretKey {
            access_key: v["accessKey"]
                .as_str()
                .ok_or(ProviderError::Unknown)?
                .into(),
            secret_key: v["secretKey"]
                .as_str()
                .ok_or(ProviderError::Unknown)?
                .into(),
        })
    }
    pub async fn revoke_key(&self, key: &str) -> ProviderResult<()> {
        self.command(&["admin", "user", "svcacct", "rm", "local", key], None)
            .await?;
        Ok(())
    }
    pub async fn delete(&self, b: &Binding) -> ProviderResult<()> {
        self.command(
            &[
                "admin", "policy", "detach", "local", &b.user, "--user", &b.user,
            ],
            None,
        )
        .await?;
        self.command(&["admin", "user", "rm", "local", &b.user], None)
            .await?;
        self.command(&["admin", "policy", "rm", "local", &b.user], None)
            .await?;
        let path = self
            .config_dir
            .join(format!("{}.policy.json", b.id.simple()));
        remove_policy_cache_file(&path)?;
        Ok(())
    }
}
fn remove_policy_cache_file(path: &std::path::Path) -> ProviderResult<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(ProviderError::Unknown),
    }
}

fn no_such_user(v: &Value) -> bool {
    match v {
        Value::Object(map) => map.iter().any(|(k, v)| {
            ((k == "Code" || k == "code")
                && matches!(v.as_str(), Some("XMinioAdminNoSuchUser" | "NoSuchUser")))
                || no_such_user(v)
        }),
        Value::Array(values) => values.iter().any(no_such_user),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn policy_cache_cleanup_accepts_missing_files_but_preserves_other_errors() {
        use std::os::unix::fs::DirBuilderExt;
        let root = std::env::temp_dir().join(format!("wd-mc-policy-test-{}", uuid::Uuid::new_v4()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
        let outcome = std::panic::catch_unwind(|| {
            let file = root.join("owned.policy.json");
            assert_eq!(remove_policy_cache_file(&file), Ok(()));
            std::fs::write(&file, b"{}").unwrap();
            assert_eq!(remove_policy_cache_file(&file), Ok(()));
            assert!(!file.exists());
            let directory = root.join("unexpected-directory");
            std::fs::create_dir(&directory).unwrap();
            assert_eq!(
                remove_policy_cache_file(&directory),
                Err(ProviderError::Unknown)
            );
            assert!(directory.is_dir());
        });
        std::fs::remove_dir_all(root).unwrap();
        if let Err(error) = outcome {
            std::panic::resume_unwind(error);
        }
    }
}
