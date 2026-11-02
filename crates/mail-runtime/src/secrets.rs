//! Retained Mail format: raw base64 key and namespaced AAD, distinct from platform secrets.
use crate::{Error, Result};
use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use serde::{Serialize, de::DeserializeOwned};
pub const SEALED_LIMIT: usize = 64 * 1024 * 1024;
pub struct MailCipher(Aes256Gcm);
/// Existing managed-private payload key derivation, shared with operator recovery.
/// This preserves the retained plugin v1 format; no fields are re-encrypted.
pub fn private_payload_key_from_hex(retained_hex: &str) -> Result<String> {
    use sha2::{Digest, Sha256};
    let key = hex::decode(retained_hex).map_err(|_| Error::Secret)?;
    if key.len() != 32 {
        return Err(Error::Secret);
    }
    let mut hash = Sha256::new();
    hash.update(b"webdock-mail:private-payload-key:v1\0");
    hash.update(key);
    Ok(STANDARD.encode(hash.finalize()))
}
pub(crate) fn key_bytes(key: &str) -> Result<[u8; 32]> {
    if key.len() != 44 {
        return Err(Error::Secret);
    }
    let decoded = STANDARD.decode(key).map_err(|_| Error::Secret)?;
    if STANDARD.encode(&decoded) != key {
        return Err(Error::Secret);
    }
    decoded.try_into().map_err(|_| Error::Secret)
}
impl MailCipher {
    pub fn new(key: &str) -> Result<Self> {
        Ok(Self(
            Aes256Gcm::new_from_slice(&key_bytes(key)?).map_err(|_| Error::Secret)?,
        ))
    }
    pub fn seal<T: Serialize>(&self, value: &T, purpose: &str) -> Result<String> {
        let aad = aad(purpose)?;
        let plain = serde_json::to_vec(value).map_err(|_| Error::Secret)?;
        if plain.len() > (SEALED_LIMIT - 44) / 4 * 3 {
            return Err(Error::Secret);
        }
        let mut nonce = [0u8; 12];
        getrandom::getrandom(&mut nonce).map_err(|_| Error::Secret)?;
        let encrypted = self
            .0
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &plain,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| Error::Secret)?;
        let (payload, tag) = encrypted.split_at(encrypted.len() - 16);
        Ok(format!(
            "v1.{}.{}.{}",
            URL_SAFE_NO_PAD.encode(nonce),
            URL_SAFE_NO_PAD.encode(tag),
            URL_SAFE_NO_PAD.encode(payload)
        ))
    }
    pub fn open<T: DeserializeOwned>(&self, value: &str, purpose: &str) -> Result<T> {
        if value.len() > SEALED_LIMIT {
            return Err(Error::Secret);
        }
        let aad = aad(purpose)?;
        let fields: Vec<_> = value.split('.').collect();
        if fields.len() != 4 || fields[0] != "v1" {
            return Err(Error::Secret);
        }
        let parts = fields[1..]
            .iter()
            .map(|part| {
                let decoded = URL_SAFE_NO_PAD.decode(part).map_err(|_| Error::Secret)?;
                if part.is_empty() || URL_SAFE_NO_PAD.encode(&decoded) != *part {
                    return Err(Error::Secret);
                }
                Ok(decoded)
            })
            .collect::<Result<Vec<_>>>()?;
        if parts[0].len() != 12 || parts[1].len() != 16 {
            return Err(Error::Secret);
        }
        let mut encrypted = parts[2].clone();
        encrypted.extend_from_slice(&parts[1]);
        let plain = self
            .0
            .decrypt(
                Nonce::from_slice(&parts[0]),
                Payload {
                    msg: &encrypted,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| Error::Secret)?;
        serde_json::from_slice(&plain).map_err(|_| Error::Secret)
    }
}
fn aad(purpose: &str) -> Result<String> {
    if purpose.is_empty() || purpose.len() > 4096 {
        return Err(Error::Secret);
    }
    Ok(format!("webdock-mail:v1:{purpose}"))
}
