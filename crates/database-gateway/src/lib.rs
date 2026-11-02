pub mod policy;

#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct Error {
    pub status: u16,
    pub message: &'static str,
}
impl Error {
    pub const fn new(status: u16, message: &'static str) -> Self {
        Self { status, message }
    }
}
pub type Result<T> = std::result::Result<T, Error>;

pub mod control;

pub mod managed;
pub(crate) fn random_token() -> Result<String> {
    use base64::Engine;
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes)
        .map_err(|_| Error::new(503, "Secure randomness unavailable."))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

pub mod server;

pub mod trust;
