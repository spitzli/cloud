use super::{Error, Result};
use std::{
    collections::BTreeMap,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    sync::Arc,
    time::Duration,
};
use tokio_postgres::{
    Config,
    config::{Host, SslMode},
};
use tokio_postgres_rustls::MakeRustlsConnect;

fn file(path: &str, private: bool) -> Result<Vec<u8>> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return Err(Error::Configuration);
    }
    let handle = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(path)
        .map_err(|_| Error::Configuration)?;
    let metadata = handle.metadata().map_err(|_| Error::Configuration)?;
    if !metadata.is_file()
        || ![0, rustix::process::geteuid().as_raw()].contains(&metadata.uid())
        || metadata.mode() & if private { 0o077 } else { 0o022 } != 0
        || metadata.len() > 1024 * 1024
    {
        return Err(Error::Configuration);
    }
    let mut data = Vec::new();
    handle
        .take(1024 * 1024 + 1)
        .read_to_end(&mut data)
        .map_err(|_| Error::Configuration)?;
    if data.len() > 1024 * 1024 {
        return Err(Error::Configuration);
    }
    Ok(data)
}
fn certificates(data: &[u8]) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>> {
    let result = rustls_pemfile::certs(&mut &*data)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| Error::Configuration)?;
    if result.is_empty() {
        return Err(Error::Configuration);
    }
    Ok(result)
}

pub(super) fn connection(
    raw: &str,
    role: &str,
    production: bool,
    timeout: Duration,
) -> Result<(Config, Option<MakeRustlsConnect>)> {
    if !crate::roles::LOGIN_ROLES.contains(&role) {
        return Err(Error::Configuration);
    }
    let canonical = crate::connection::ConnectionUrl::parse(raw, production)
        .map_err(|_| Error::Configuration)?;
    let mut url = url::Url::parse(canonical.as_str()).map_err(|_| Error::Configuration)?;
    let host = match url.host().ok_or(Error::Configuration)? {
        url::Host::Domain(name) => name.into(),
        url::Host::Ipv4(ip) => ip.to_string(),
        url::Host::Ipv6(ip) => ip.to_string(),
    };
    let mut options = BTreeMap::new();
    for (key, value) in url.query_pairs() {
        let key = match key.as_ref() {
            "sslmode" | "ssl-mode" => "mode",
            "sslrootcert" | "ssl-root-cert" | "ssl-ca" => "root",
            "sslcert" | "ssl-cert" => "cert",
            "sslkey" | "ssl-key" => "key",
            "application_name" => "name",
            _ => return Err(Error::Configuration),
        };
        if options.insert(key, value.into_owned()).is_some() {
            return Err(Error::Configuration);
        }
    }
    let mode = options
        .get("mode")
        .map(String::as_str)
        .unwrap_or("prefer")
        .to_ascii_lowercase();
    let tls = matches!(mode.as_str(), "require" | "verify-ca" | "verify-full")
        || options.contains_key("root")
        || options.contains_key("cert")
        || options.contains_key("key");
    if mode == "disable" && tls {
        return Err(Error::Configuration);
    }
    url.set_query(None);
    let mut config: Config = url.as_str().parse().map_err(|_| Error::Configuration)?;
    if config.get_user() != Some(role)
        || config.get_dbname() != Some(url.path().trim_start_matches('/'))
        || config.get_ports() != [url.port().unwrap_or(5432)]
        || !matches!(config.get_hosts(),[Host::Tcp(name)]if name==&host)
    {
        return Err(Error::Configuration);
    }
    config.connect_timeout(timeout).application_name(
        options
            .get("name")
            .map(String::as_str)
            .unwrap_or("spitzli-native-postgres"),
    );
    if !tls {
        config.ssl_mode(SslMode::Disable);
        return Ok((config, None));
    }
    config.ssl_mode(SslMode::Require);
    let mut roots = rustls::RootCertStore::empty();
    if let Some(path) = options.get("root") {
        for cert in certificates(&file(path, false)?)? {
            roots.add(cert).map_err(|_| Error::Configuration)?;
        }
    } else {
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }
    let builder = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| Error::Configuration)?
    .with_root_certificates(roots);
    let tls = match (options.get("cert"), options.get("key")) {
        (None, None) => builder.with_no_client_auth(),
        (Some(cert), Some(key)) => {
            let data = file(key, true)?;
            let mut reader = &data[..];
            let key = rustls_pemfile::private_key(&mut reader)
                .map_err(|_| Error::Configuration)?
                .ok_or(Error::Configuration)?;
            if rustls_pemfile::private_key(&mut reader)
                .map_err(|_| Error::Configuration)?
                .is_some()
            {
                return Err(Error::Configuration);
            }
            builder
                .with_client_auth_cert(certificates(&file(cert, false)?)?, key)
                .map_err(|_| Error::Configuration)?
        }
        _ => return Err(Error::Configuration),
    };
    Ok((config, Some(MakeRustlsConnect::new(tls))))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_tls_modes_never_downgrade_case_variants() {
        for mode in ["verify-full", "VERIFY-FULL", "Verify-Full"] {
            let (config, tls) = connection(
                &format!("postgres://webdock_auth_runtime:private@db.invalid/db?sslmode={mode}"),
                "webdock_auth_runtime",
                true,
                Duration::from_secs(5),
            )
            .unwrap();
            assert!(tls.is_some());
            assert_eq!(config.get_ssl_mode(), SslMode::Require);
        }
    }
    #[test]
    fn native_tls_rejects_fifo_without_blocking() {
        const CHILD: &str = "SPITZLI_OWNED_TLS_FIFO_TEST";
        if let Some(path) = std::env::var_os(CHILD) {
            assert!(file(path.to_str().unwrap(), false).is_err());
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fifo");
        rustix::fs::mkfifoat(
            rustix::fs::CWD,
            &path,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )
        .unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "postgres::config::tests::native_tls_rejects_fifo_without_blocking",
            ])
            .env(CHILD, &path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                return;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("TLS file validation blocked on a FIFO");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn native_tls_files_enforce_regular_private_bounded_inputs() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("key");
        std::fs::write(&path, b"owned private test data").unwrap();
        let permissions =
            |mode| std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        permissions(0o600);
        assert!(file(path.to_str().unwrap(), true).is_ok());
        permissions(0o644);
        assert!(file(path.to_str().unwrap(), false).is_ok());
        assert!(file(path.to_str().unwrap(), true).is_err());
        permissions(0o620);
        assert!(file(path.to_str().unwrap(), false).is_err());
        permissions(0o600);
        let link = directory.path().join("link");
        symlink(&path, &link).unwrap();
        assert!(file(link.to_str().unwrap(), false).is_err());
        assert!(file(directory.path().to_str().unwrap(), false).is_err());
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(1024 * 1024 + 1)
            .unwrap();
        assert!(file(path.to_str().unwrap(), true).is_err());
    }
}
