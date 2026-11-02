use crate::{IdentityError, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce, aead::Aead};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use subtle::ConstantTimeEq;
use unicode_normalization::UnicodeNormalization;

type HmacSha256 = Hmac<Sha256>;
pub(crate) fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    getrandom::getrandom(&mut bytes).map_err(|_| IdentityError::Crypto)?;
    Ok(bytes)
}
pub(crate) fn mac(secret: &[u8], value: &[u8]) -> Vec<u8> {
    let mut signer =
        <HmacSha256 as Mac>::new_from_slice(secret).expect("HMAC accepts any key length");
    signer.update(value);
    signer.finalize().into_bytes().to_vec()
}
fn derive(password: &str, salt: &str) -> Result<[u8; 64]> {
    let mut key = [0; 64];
    let params = scrypt::Params::new(14, 16, 1, 64).map_err(|_| IdentityError::Crypto)?;
    scrypt::scrypt(
        password.nfkc().collect::<String>().as_bytes(),
        salt.as_bytes(),
        &params,
        &mut key,
    )
    .map_err(|_| IdentityError::Crypto)?;
    Ok(key)
}
pub fn hash_password(password: &str) -> Result<String> {
    if password.chars().count() < 12 || password.chars().count() > 128 {
        return Err(IdentityError::InvalidInput);
    }
    let salt = hex::encode(random::<16>()?);
    Ok(format!("{salt}:{}", hex::encode(derive(password, &salt)?)))
}
pub fn verify_password(password: &str, encoded: &str) -> Result<bool> {
    if password.len() > 4096 {
        return Ok(false);
    }
    let (salt, key) = encoded
        .split_once(':')
        .ok_or(IdentityError::InvalidEncoding)?;
    if salt.len() != 32
        || key.len() != 128
        || !encoded
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c) || c == b':')
    {
        return Err(IdentityError::InvalidEncoding);
    }
    let expected = hex::decode(key).map_err(|_| IdentityError::InvalidEncoding)?;
    Ok(bool::from(
        derive(password, salt)?.ct_eq(expected.as_slice()),
    ))
}

/// Contains key material; deliberately does not implement Debug or Serialize.
pub struct CookiePolicy {
    secure: bool,
    secret: String,
}
impl CookiePolicy {
    pub fn new(secure: bool, secret: impl Into<String>) -> Result<Self> {
        let secret = secret.into();
        if secret.len() < 32 {
            return Err(IdentityError::Configuration);
        }
        Ok(Self { secure, secret })
    }
    pub fn name(&self) -> &'static str {
        if self.secure {
            "__Host-webdock-auth.session_token"
        } else {
            "webdock-auth.session_token"
        }
    }
    pub fn sign(&self, token: &str) -> Result<String> {
        if token.is_empty()
            || token.len() > 512
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            return Err(IdentityError::InvalidEncoding);
        }
        // encodeURIComponent permits '.', '-' and '_' in these token/signature alphabets.
        Ok(format!(
            "{token}.{}",
            STANDARD
                .encode(mac(self.secret.as_bytes(), token.as_bytes()))
                .replace('+', "%2B")
                .replace('/', "%2F")
                .replace('=', "%3D")
        ))
    }
    pub fn read(&self, header: &str) -> Result<Option<String>> {
        self.read_named(header, self.name())
    }
    pub(crate) fn read_named(&self, header: &str, name: &str) -> Result<Option<String>> {
        if header.len() > 16384 {
            return Ok(None);
        }
        let prefix = format!("{name}=");
        let mut values = header
            .split(';')
            .filter_map(|p| p.trim().strip_prefix(&prefix));
        let Some(value) = values.next() else {
            return Ok(None);
        };
        if values.next().is_some() || value.len() > 1024 {
            return Ok(None);
        }
        let Ok(decoded) = percent_encoding::percent_decode_str(value).decode_utf8() else {
            return Ok(None);
        };
        let Some((token, signature)) = decoded.rsplit_once('.') else {
            return Ok(None);
        };
        if token.is_empty()
            || token.len() > 512
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            return Ok(None);
        }
        let Ok(signature) = STANDARD.decode(signature) else {
            return Ok(None);
        };
        let mut signer = <HmacSha256 as Mac>::new_from_slice(self.secret.as_bytes())
            .map_err(|_| IdentityError::Crypto)?;
        signer.update(token.as_bytes());
        Ok(signer
            .verify_slice(&signature)
            .is_ok()
            .then(|| token.to_owned()))
    }
    pub fn header(&self, token: &str, max_age: u32) -> Result<String> {
        Ok(format!(
            "{}={}{}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}",
            self.name(),
            self.sign(token)?,
            if self.secure { "; Secure" } else { "" }
        ))
    }

    pub(crate) fn challenge_name(&self) -> &'static str {
        if self.secure {
            "__Host-webdock-auth.two_factor"
        } else {
            "webdock-auth.two_factor"
        }
    }
    pub(crate) fn read_challenge(&self, header: &str) -> Result<Option<String>> {
        self.read_named(header, self.challenge_name())
    }
    pub(crate) fn challenge_header(&self, token: &str) -> Result<String> {
        Ok(self
            .header(token, 300)?
            .replacen(self.name(), self.challenge_name(), 1))
    }
    pub fn clear_challenge_header(&self) -> String {
        self.clear_header()
            .replacen(self.name(), self.challenge_name(), 1)
    }
    pub(crate) fn passkey_name(&self) -> &'static str {
        if self.secure {
            "__Host-webdock-auth.better-auth-passkey"
        } else {
            "webdock-auth.better-auth-passkey"
        }
    }
    pub(crate) fn passkey_header(&self, token: &str) -> Result<String> {
        Ok(self
            .header(token, 300)?
            .replacen(self.name(), self.passkey_name(), 1))
    }
    pub(crate) fn clear_passkey_header(&self) -> String {
        self.clear_header()
            .replacen(self.name(), self.passkey_name(), 1)
    }
    pub fn clear_header(&self) -> String {
        format!(
            "{}={}{}; HttpOnly; SameSite=Lax; Path=/; Max-Age=0",
            self.name(),
            "",
            if self.secure { "; Secure" } else { "" }
        )
    }
}

/// Version keys and legacy fallback must be supplied explicitly by installation configuration.
pub struct SecretKeys {
    keys: BTreeMap<u32, String>,
    current: u32,
    legacy: Option<String>,
}
impl SecretKeys {
    pub fn new(keys: BTreeMap<u32, String>, current: u32, legacy: Option<String>) -> Result<Self> {
        if !keys.contains_key(&current)
            || keys.values().any(|s| s.len() < 32)
            || legacy.as_ref().is_some_and(|s| s.len() < 32)
        {
            return Err(IdentityError::Configuration);
        }
        Ok(Self {
            keys,
            current,
            legacy,
        })
    }
    pub fn encrypt(&self, plaintext: &str) -> Result<String> {
        if plaintext.len() > 65536 {
            return Err(IdentityError::InvalidInput);
        }
        let nonce = random::<24>()?;
        let key = Sha256::digest(self.keys[&self.current].as_bytes());
        let cipher = XChaCha20Poly1305::new(&key);
        let ciphertext = cipher
            .encrypt(XNonce::from_slice(&nonce), plaintext.as_bytes())
            .map_err(|_| IdentityError::Crypto)?;
        let mut data = nonce.to_vec();
        data.extend(ciphertext);
        Ok(format!("$ba${}${}", self.current, hex::encode(data)))
    }
    pub fn decrypt(&self, encoded: &str) -> Result<String> {
        if encoded.len() > 132000 {
            return Err(IdentityError::InvalidEncoding);
        }
        let (secret, hex) = if let Some(envelope) = encoded.strip_prefix("$ba$") {
            let (version, hex) = envelope
                .split_once('$')
                .ok_or(IdentityError::InvalidEncoding)?;
            let version = version
                .parse::<u32>()
                .map_err(|_| IdentityError::InvalidEncoding)?;
            (
                self.keys
                    .get(&version)
                    .ok_or(IdentityError::Configuration)?,
                hex,
            )
        } else {
            (
                self.legacy.as_ref().ok_or(IdentityError::Configuration)?,
                encoded,
            )
        };
        let data = hex::decode(hex).map_err(|_| IdentityError::InvalidEncoding)?;
        if data.len() < 40 {
            return Err(IdentityError::InvalidEncoding);
        }
        let key = Sha256::digest(secret.as_bytes());
        let cipher = XChaCha20Poly1305::new(&key);
        let plaintext = cipher
            .decrypt(XNonce::from_slice(&data[..24]), &data[24..])
            .map_err(|_| IdentityError::Crypto)?;
        String::from_utf8(plaintext).map_err(|_| IdentityError::InvalidEncoding)
    }
}
