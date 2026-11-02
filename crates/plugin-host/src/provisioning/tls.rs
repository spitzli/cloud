use super::{ProvisioningError as Error, Result};
use rcgen::{CertificateParams, ExtendedKeyUsagePurpose, Issuer, KeyPair, KeyUsagePurpose};
use sha2::{Digest, Sha256};
use std::time::Duration;

/// Private leaf material; intentionally no Debug or Serialize implementation.
pub struct RuntimeIdentity {
    pub certificate_pem: String,
    pub private_key_pem: String,
    pub certificate_sha256: String,
}
pub fn issue_runtime_identity(
    ca_pem: &str,
    ca_key_pem: &str,
    dns_name: &str,
) -> Result<RuntimeIdentity> {
    if ca_pem.len() > 65536
        || ca_key_pem.len() > 65536
        || dns_name.len() > 253
        || dns_name.is_empty()
        || dns_name.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || !label.as_bytes()[0].is_ascii_alphanumeric()
                || !label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                || !label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        || dns_name.parse::<std::net::IpAddr>().is_ok()
    {
        return Err(Error::Invalid);
    }
    let (_, pem) = x509_parser::pem::parse_x509_pem(ca_pem.as_bytes()).map_err(|_| Error::Trust)?;
    let (rest, ca) =
        x509_parser::parse_x509_certificate(&pem.contents).map_err(|_| Error::Trust)?;
    let key = KeyPair::from_pem(ca_key_pem).map_err(|_| Error::Trust)?;
    if !rest.is_empty()
        || !ca.is_ca()
        || !ca.validity().is_valid()
        || ca.public_key().subject_public_key.data.as_ref() != key.public_key_raw()
        || !ca
            .key_usage()
            .map_err(|_| Error::Trust)?
            .is_some_and(|u| u.value.key_cert_sign())
    {
        return Err(Error::Trust);
    }
    let issuer = Issuer::from_ca_cert_pem(ca_pem, key).map_err(|_| Error::Trust)?;
    let leaf_key = KeyPair::generate().map_err(|_| Error::Unavailable)?;
    let mut params = CertificateParams::new(vec![dns_name.into()]).map_err(|_| Error::Invalid)?;
    let now = x509_parser::time::ASN1Time::now().to_datetime();
    params.not_before = (now - Duration::from_secs(60)).max(ca.validity().not_before.to_datetime());
    params.not_after =
        (now + Duration::from_secs(30 * 24 * 60 * 60)).min(ca.validity().not_after.to_datetime());
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![
        ExtendedKeyUsagePurpose::ServerAuth,
        ExtendedKeyUsagePurpose::ClientAuth,
    ];
    let leaf = params
        .signed_by(&leaf_key, &issuer)
        .map_err(|_| Error::Unavailable)?;
    Ok(RuntimeIdentity {
        certificate_sha256: hex::encode(Sha256::digest(leaf.der())),
        certificate_pem: leaf.pem(),
        private_key_pem: leaf_key.serialize_pem(),
    })
}
