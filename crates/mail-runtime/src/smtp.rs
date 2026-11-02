//! One bounded attempt to the assigned private Mail engine. SMTP parsing, capabilities and
//! transparency use mail-send/smtp-proto; TLS and signature verification use Rustls.
use crate::{
    message::{MAX_MESSAGE, envelope_address},
    proof::valid_id,
};
use mail_send::{Credentials, SmtpClient};
use rustls::{
    DigitallySignedStruct, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use sha2::{Digest, Sha256};
use smtp_proto::{AUTH_PLAIN, EXT_8BIT_MIME, EXT_SMTP_UTF8};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use x509_parser::prelude::FromDer;

/// No Debug implementation: this value contains a scoped submission credential.
pub struct SmtpOptions {
    host: String,
    port: u16,
    pub certificate_sha256: String,
    username: String,
    password: String,
    timeout: Duration,
}
impl SmtpOptions {
    pub fn assigned(
        customer: &str,
        certificate_sha256: String,
        username: String,
        password: String,
        timeout: Duration,
    ) -> Result<Self, SmtpFailure> {
        if !valid_id(customer) {
            return Err(SmtpFailure::before());
        }
        let value = Self {
            host: format!("app-{customer}.wd-mail-{customer}.svc.cluster.local"),
            port: 2465,
            certificate_sha256,
            username,
            password,
            timeout,
        };
        value.validate()?;
        Ok(value)
    }
    #[cfg(feature = "test-support")]
    pub fn local_fixture(
        port: u16,
        certificate_sha256: String,
        username: String,
        password: String,
        timeout: Duration,
    ) -> Result<Self, SmtpFailure> {
        let value = Self {
            host: "127.0.0.1".into(),
            port,
            certificate_sha256,
            username,
            password,
            timeout,
        };
        value.validate()?;
        Ok(value)
    }
    fn validate(&self) -> Result<(), SmtpFailure> {
        if self.port == 0
            || self.certificate_sha256.len() != 64
            || !self
                .certificate_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || [&self.username, &self.password].iter().any(|s| {
                s.is_empty() || s.encode_utf16().count() > 1024 || s.chars().any(|c| c <= '\u{1f}')
            })
            || self.timeout < Duration::from_millis(100)
            || self.timeout > Duration::from_secs(120)
        {
            return Err(SmtpFailure::before());
        }
        Ok(())
    }
}
#[derive(Debug, thiserror::Error)]
#[error("SMTP acceptance uncertain={uncertain}; no automatic transport retry")]
pub struct SmtpFailure {
    pub uncertain: bool,
    pub response_code: Option<u16>,
}
impl SmtpFailure {
    fn before() -> Self {
        Self {
            uncertain: false,
            response_code: None,
        }
    }
    fn transport(error: mail_send::Error, started: bool) -> Self {
        let code = match error {
            mail_send::Error::UnexpectedReply(reply)
            | mail_send::Error::AuthenticationFailed(reply) => Some(reply.code),
            _ => None,
        };
        Self {
            uncertain: started && !code.is_some_and(|c| (400..600).contains(&c)),
            response_code: code,
        }
    }
}
fn expect(
    reply: smtp_proto::Response<String>,
    code: u16,
    started: bool,
) -> Result<(), SmtpFailure> {
    if reply.code == code {
        Ok(())
    } else {
        Err(SmtpFailure::transport(
            mail_send::Error::UnexpectedReply(reply),
            started,
        ))
    }
}

pub async fn submit(
    options: &SmtpOptions,
    from: &str,
    to: &[String],
    raw: &[u8],
) -> Result<(), SmtpFailure> {
    options.validate()?;
    if envelope_address(from).is_err()
        || to.is_empty()
        || to.len() > 100
        || to.iter().any(|s| envelope_address(s).is_err())
        || raw.len() > MAX_MESSAGE
        || raw.contains(&0)
        || !raw.windows(4).any(|w| w == b"\r\n\r\n")
        || raw.iter().enumerate().any(|(i, b)| {
            (*b == b'\n' && (i == 0 || raw[i - 1] != b'\r'))
                || (*b == b'\r' && raw.get(i + 1) != Some(&b'\n'))
        })
    {
        return Err(SmtpFailure::before());
    }
    let started = AtomicBool::new(false);
    tokio::time::timeout(options.timeout, async {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let pin: [u8; 32] = hex::decode(&options.certificate_sha256)
            .map_err(|_| SmtpFailure::before())?
            .try_into()
            .map_err(|_| SmtpFailure::before())?;
        let verifier = Arc::new(PinnedCertificate {
            pin,
            provider: provider.clone(),
        });
        let config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|_| SmtpFailure::before())?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth();
        let tcp = TcpStream::connect((options.host.as_str(), options.port))
            .await
            .map_err(|_| SmtpFailure::before())?;
        let name = ServerName::try_from(options.host.clone()).map_err(|_| SmtpFailure::before())?;
        let stream = TlsConnector::from(Arc::new(config))
            .connect(name, tcp)
            .await
            .map_err(|_| SmtpFailure::before())?;
        let mut client = SmtpClient {
            stream,
            timeout: options.timeout,
        };
        deliver(
            &mut client,
            Some((&options.username, &options.password)),
            from,
            to,
            raw,
            &started,
        )
        .await
    })
    .await
    .unwrap_or_else(|_| {
        Err(SmtpFailure {
            uncertain: started.load(Ordering::Relaxed),
            response_code: None,
        })
    })
}

#[derive(Debug)]
struct PinnedCertificate {
    pin: [u8; 32],
    provider: Arc<rustls::crypto::CryptoProvider>,
}
impl ServerCertVerifier for PinnedCertificate {
    fn verify_server_cert(
        &self,
        end: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _name: &ServerName<'_>,
        _ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        // The exact controller-issued DER pin is the trust anchor for this private self-signed
        // instance; DNS or a publicly trusted certificate cannot substitute for the assigned key.
        let digest: [u8; 32] = Sha256::digest(end.as_ref()).into();
        let (_, cert) =
            x509_parser::certificate::X509Certificate::from_der(end.as_ref()).map_err(|_| {
                rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding)
            })?;
        let at = x509_parser::time::ASN1Time::from_timestamp(now.as_secs() as i64)
            .map_err(|_| rustls::Error::General("invalid certificate time".into()))?;
        if digest != self.pin || !cert.validity().is_valid_at(at) {
            return Err(rustls::Error::General(
                "private Mail certificate does not match its current assignment".into(),
            ));
        }
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            signature,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            signature,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Explicit local-only adapter; phase/acceptance classification is identical to private SMTP.
pub async fn submit_loopback(
    host: &str,
    port: u16,
    from: &str,
    to: &[String],
    raw: &[u8],
    timeout: Duration,
) -> Result<(), SmtpFailure> {
    if !matches!(host, "127.0.0.1" | "::1" | "localhost")
        || port == 0
        || timeout < Duration::from_millis(100)
        || timeout > Duration::from_secs(120)
        || envelope_address(from).is_err()
        || to.is_empty()
        || to.len() > 100
        || to.iter().any(|s| envelope_address(s).is_err())
        || raw.is_empty()
        || raw.len() > MAX_MESSAGE
        || raw.contains(&0)
        || !raw.windows(4).any(|w| w == b"\r\n\r\n")
        || raw.iter().enumerate().any(|(i, b)| {
            (*b == b'\n' && (i == 0 || raw[i - 1] != b'\r'))
                || (*b == b'\r' && raw.get(i + 1) != Some(&b'\n'))
        })
    {
        return Err(SmtpFailure::before());
    }
    let started = AtomicBool::new(false);
    tokio::time::timeout(timeout, async {
        let stream = TcpStream::connect((host, port))
            .await
            .map_err(|_| SmtpFailure::before())?;
        let mut client = SmtpClient { stream, timeout };
        deliver(&mut client, None, from, to, raw, &started).await
    })
    .await
    .unwrap_or_else(|_| {
        Err(SmtpFailure {
            uncertain: started.load(Ordering::Relaxed),
            response_code: None,
        })
    })
}
async fn deliver<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    client: &mut SmtpClient<S>,
    credentials: Option<(&str, &str)>,
    from: &str,
    to: &[String],
    raw: &[u8],
    started: &AtomicBool,
) -> Result<(), SmtpFailure> {
    expect(
        client
            .read()
            .await
            .map_err(|e| SmtpFailure::transport(e, false))?,
        220,
        false,
    )?;
    let mut capabilities = client
        .ehlo("webdock.invalid")
        .await
        .map_err(|e| SmtpFailure::transport(e, false))?;
    if let Some((username, password)) = credentials {
        if capabilities.auth_mechanisms & AUTH_PLAIN == 0 {
            return Err(SmtpFailure::before());
        }
        capabilities.auth_mechanisms = AUTH_PLAIN;
        client
            .authenticate(
                Credentials::Plain {
                    username,
                    secret: password,
                },
                &capabilities,
            )
            .await
            .map_err(|e| SmtpFailure::transport(e, false))?;
    }
    let eight_bit = !raw.is_ascii();
    let utf8 = eight_bit || !from.is_ascii() || to.iter().any(|s| !s.is_ascii());
    if (utf8 && capabilities.capabilities & EXT_SMTP_UTF8 == 0)
        || (eight_bit && capabilities.capabilities & EXT_8BIT_MIME == 0)
    {
        return Err(SmtpFailure::before());
    }
    let command = format!(
        "MAIL FROM:<{from}>{}{}\r\n",
        if utf8 { " SMTPUTF8" } else { "" },
        if eight_bit { " BODY=8BITMIME" } else { "" }
    );
    expect(
        client
            .cmd(command)
            .await
            .map_err(|e| SmtpFailure::transport(e, false))?,
        250,
        false,
    )?;
    let mut seen = std::collections::HashSet::new();
    for recipient in to {
        if seen.insert(recipient) {
            expect(
                client
                    .cmd(format!("RCPT TO:<{recipient}>\r\n"))
                    .await
                    .map_err(|e| SmtpFailure::transport(e, false))?,
                250,
                false,
            )?;
        }
    }
    expect(
        client
            .cmd(b"DATA\r\n")
            .await
            .map_err(|e| SmtpFailure::transport(e, false))?,
        354,
        false,
    )?;
    // From this point a timeout/disconnect is ambiguous, even before the last byte is written.
    started.store(true, Ordering::Relaxed);
    client.write_message(raw).await.map_err(|_| SmtpFailure {
        uncertain: true,
        response_code: None,
    })?;
    expect(
        client
            .read()
            .await
            .map_err(|e| SmtpFailure::transport(e, true))?,
        250,
        true,
    )
}
