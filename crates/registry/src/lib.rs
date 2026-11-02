//! Native registry CRUD and compatibility queries on the retained RLS-protected schema.
pub mod contracts;
mod entities;
mod query;
mod service;
pub use service::{
    Allocator, ArchiveGuard, DatabaseAllocator, RegistryService, RetainedArchiveGuard,
};
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("{0}")]
    Forbidden(&'static str),
    #[error("{0}")]
    NotFound(&'static str),
    #[error("{0}")]
    Conflict(&'static str),
    #[error("registry database operation failed")]
    Database(sea_orm::DbErr),
    #[error("ID allocation is unavailable")]
    Allocation(#[from] webdock_persistence::Error),
    #[error("identity context is unavailable")]
    Identity(#[from] webdock_identity::IdentityError),
}
impl RegistryError {
    pub fn status(&self) -> u16 {
        match self {
            Self::Invalid(_) => 400,
            Self::Forbidden(_) => 403,
            Self::NotFound(_) => 404,
            Self::Conflict(_) => 409,
            Self::Identity(_) => 403,
            Self::Database(_) | Self::Allocation(_) => 500,
        }
    }
}
impl From<sea_orm::DbErr> for RegistryError {
    fn from(error: sea_orm::DbErr) -> Self {
        if error
            .sql_err()
            .is_some_and(|e| matches!(e, sea_orm::SqlErr::UniqueConstraintViolation(_)))
        {
            return Self::Conflict("Registry record conflicts with existing data.");
        }
        // PostgreSQL's retained deletion fence uses object_not_in_prerequisite_state.
        if let sea_orm::DbErr::Query(sea_orm::RuntimeErr::SqlxError(e))
        | sea_orm::DbErr::Exec(sea_orm::RuntimeErr::SqlxError(e)) = &error
            && e.as_database_error()
                .and_then(|e| e.code())
                .is_some_and(|code| code == "55000")
        {
            return Self::Conflict("Project deletion is in progress. Wait for its result.");
        }
        if let sea_orm::DbErr::Query(sea_orm::RuntimeErr::SqlxError(e))
        | sea_orm::DbErr::Exec(sea_orm::RuntimeErr::SqlxError(e)) = &error
            && e.as_database_error()
                .and_then(|e| e.code())
                .is_some_and(|code| code == "WD429")
        {
            return Self::Conflict(
                "The customer's plan allows no more projects. Raise its project limit first.",
            );
        }
        if let sea_orm::DbErr::Query(sea_orm::RuntimeErr::SqlxError(e))
        | sea_orm::DbErr::Exec(sea_orm::RuntimeErr::SqlxError(e)) = &error
            && e.as_database_error()
                .and_then(|e| e.code())
                .is_some_and(|code| code == "WD051")
        {
            return Self::Conflict(
                "Suspend PostgreSQL recurrence, WAL and retention and wait for native observation before archival.",
            );
        }
        Self::Database(error)
    }
}
pub type Result<T> = std::result::Result<T, RegistryError>;
