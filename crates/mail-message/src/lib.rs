//! Shared bounded message representation and MIME sanitation; no database or provider dependency.
mod message;
pub use message::*;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid Mail message")]
    Invalid,
}
pub type Result<T> = std::result::Result<T, Error>;
