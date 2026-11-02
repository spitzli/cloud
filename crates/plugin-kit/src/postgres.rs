//! PostgreSQL TLS always verifies the driver's actual hostname against the supplied CA.
use crate::Error;
use std::{sync::Arc, time::Duration};
use tokio_postgres::{
    Client, NoTls,
    config::{Host, SslMode},
};

pub async fn connect(url: &str, ca: Option<&[u8]>) -> Result<Client, Error> {
    let tls = ca.map(tls_config).transpose()?;
    if tls.is_none() {
        crate::loopback(url).map_err(|_| "plaintext storage requires loopback")?;
    }
    let mut config: tokio_postgres::Config =
        url.parse().map_err(|_| "invalid storage configuration")?;
    config.connect_timeout(Duration::from_secs(5));
    if let Some(tls) = tls {
        if config.get_hosts().is_empty()
            || config
                .get_hosts()
                .iter()
                .any(|h| !matches!(h, Host::Tcp(_)))
        {
            return Err("TLS storage requires a hostname".into());
        }
        config.ssl_mode(SslMode::Require);
        let (client, connection) = config
            .connect(tokio_postgres_rustls::MakeRustlsConnect::new(tls))
            .await
            .map_err(|_| "storage TLS connection unavailable")?;
        tokio::spawn(async move {
            if connection.await.is_err() {
                eprintln!("plugin_storage_connection_lost");
            }
        });
        Ok(client)
    } else {
        let (client, connection) = config
            .connect(NoTls)
            .await
            .map_err(|_| "storage connection unavailable")?;
        tokio::spawn(async move {
            if connection.await.is_err() {
                eprintln!("plugin_storage_connection_lost");
            }
        });
        Ok(client)
    }
}
fn tls_config(ca: &[u8]) -> Result<rustls::ClientConfig, Error> {
    let result = || -> Result<rustls::ClientConfig, Error> {
        if ca.len() > 65536 {
            return Err("CA limit".into());
        }
        let mut roots = rustls::RootCertStore::empty();
        let mut input = ca;
        for cert in rustls_pemfile::certs(&mut input) {
            roots.add(cert?)?;
        }
        if roots.is_empty() {
            return Err("CA required".into());
        }
        Ok(rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth())
    };
    result().map_err(|_| "invalid storage TLS trust".into())
}
