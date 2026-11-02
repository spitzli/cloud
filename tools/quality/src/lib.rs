pub mod architecture;
pub mod migrations;
pub mod runner;
pub mod serde_contracts;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot read {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("invalid Cargo manifest {path}: {source}")]
    Manifest {
        path: std::path::PathBuf,
        source: toml::de::Error,
    },
    #[error(
        "Cargo metadata failed for {0}; run cargo metadata --locked --all-features for that workspace"
    )]
    Metadata(std::path::PathBuf),
    #[error("missing dependency metadata: {0}")]
    MissingPackage(String),
}
pub type Result<T> = std::result::Result<T, Error>;
