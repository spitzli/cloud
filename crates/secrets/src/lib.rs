//! Compatibility codec for retained platform AES-256-GCM envelopes. Not a new cryptographic primitive.
use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("retained platform encryption is not configured")]
    Configuration,
    #[error("stored secret envelope is invalid or unavailable")]
    InvalidEnvelope,
}
pub type Result<T> = std::result::Result<T, Error>;
/// Deliberately does not implement Debug or Serialize.
pub struct SecretCipher {
    cipher: Aes256Gcm,
}
impl SecretCipher {
    pub fn new(secret: &str) -> Result<Self> {
        if secret.len() < 32 || secret.len() > 4096 {
            return Err(Error::Configuration);
        }
        let mut key = Sha256::new();
        key.update(b"webdock-mail-v1\0");
        key.update(secret.as_bytes());
        Ok(Self {
            cipher: Aes256Gcm::new_from_slice(&key.finalize()).map_err(|_| Error::Configuration)?,
        })
    }
    pub fn seal<T: Serialize>(&self, value: &T, purpose: &str) -> Result<String> {
        purpose_valid(purpose)?;
        let plaintext = serde_json::to_vec(value).map_err(|_| Error::InvalidEnvelope)?;
        if plaintext.len() > 24520 {
            return Err(Error::InvalidEnvelope);
        }
        let mut nonce = [0u8; 12];
        getrandom::getrandom(&mut nonce).map_err(|_| Error::InvalidEnvelope)?;
        let encrypted = self
            .cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &plaintext,
                    aad: purpose.as_bytes(),
                },
            )
            .map_err(|_| Error::InvalidEnvelope)?;
        let (ciphertext, tag) = encrypted.split_at(encrypted.len() - 16);
        let sealed = format!(
            "v1.{}.{}.{}",
            URL_SAFE_NO_PAD.encode(nonce),
            URL_SAFE_NO_PAD.encode(tag),
            URL_SAFE_NO_PAD.encode(ciphertext)
        );
        if sealed.len() > 32768 {
            return Err(Error::InvalidEnvelope);
        }
        Ok(sealed)
    }
    pub fn open<T: DeserializeOwned>(&self, sealed: &str, purpose: &str) -> Result<T> {
        purpose_valid(purpose)?;
        if sealed.len() > 32768 {
            return Err(Error::InvalidEnvelope);
        }
        let parts = sealed.split('.').collect::<Vec<_>>();
        if parts.len() != 4
            || parts[0] != "v1"
            || parts[1..].iter().any(|s| {
                s.is_empty()
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            })
        {
            return Err(Error::InvalidEnvelope);
        }
        let nonce = URL_SAFE_NO_PAD
            .decode(parts[1])
            .map_err(|_| Error::InvalidEnvelope)?;
        let tag = URL_SAFE_NO_PAD
            .decode(parts[2])
            .map_err(|_| Error::InvalidEnvelope)?;
        let mut ciphertext = URL_SAFE_NO_PAD
            .decode(parts[3])
            .map_err(|_| Error::InvalidEnvelope)?;
        if nonce.len() != 12 || tag.len() != 16 {
            return Err(Error::InvalidEnvelope);
        }
        ciphertext.extend(tag);
        let plaintext = self
            .cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &ciphertext,
                    aad: purpose.as_bytes(),
                },
            )
            .map_err(|_| Error::InvalidEnvelope)?;
        serde_json::from_slice(&plaintext).map_err(|_| Error::InvalidEnvelope)
    }
}
fn purpose_valid(purpose: &str) -> Result<()> {
    if purpose.is_empty() || purpose.len() > 1024 {
        return Err(Error::InvalidEnvelope);
    }
    Ok(())
}
