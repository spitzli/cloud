//! Credentials at rest: AES-256-GCM with a key from the private configuration, bound to the record id.
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};

pub struct Sealer(Aes256Gcm);
impl Sealer {
    pub fn new(key_hex: &str) -> Result<Self, &'static str> {
        let key: [u8; 32] = hex::decode(key_hex)
            .map_err(|_| "invalid secret key")?
            .try_into()
            .map_err(|_| "secret key must be 32 bytes")?;
        Ok(Self(Aes256Gcm::new(&key.into())))
    }
    pub fn seal(&self, id: &uuid::Uuid, plain: &str) -> Vec<u8> {
        let nonce = random::<12>();
        let mut out = nonce.to_vec();
        out.extend(
            self.0
                .encrypt(
                    Nonce::from_slice(&nonce),
                    Payload {
                        msg: plain.as_bytes(),
                        aad: id.as_bytes(),
                    },
                )
                .expect("AES-GCM encryption of a bounded secret cannot fail"),
        );
        out
    }
    pub fn open(&self, id: &uuid::Uuid, sealed: &[u8]) -> Option<String> {
        let (nonce, body) = sealed.split_at_checked(12)?;
        let plain = self
            .0
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: body,
                    aad: id.as_bytes(),
                },
            )
            .ok()?;
        String::from_utf8(plain).ok()
    }
}
pub fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    getrandom::getrandom(&mut bytes).expect("operating system randomness");
    bytes
}
/// Hex only, so it is safe inside a quoted SQL literal and a connection URI.
pub fn password() -> String {
    hex::encode(random::<24>())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sealed_values_are_bound_to_their_resource() {
        let sealer = Sealer::new(&"11".repeat(32)).unwrap();
        let (a, b) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        let sealed = sealer.seal(&a, "secret");
        assert_eq!(sealer.open(&a, &sealed).as_deref(), Some("secret"));
        assert_eq!(sealer.open(&b, &sealed), None);
        assert!(Sealer::new("00").is_err());
    }
}
