//! Persisted database grants and short-lived gateway authority.
mod agent_contracts;
mod binding;
mod contracts;
mod database_authority;
mod management;
mod read;
pub use read::{DATABASE_READ_TOOLS, DatabaseReadCommand, database_read_tools};
mod routes;
mod service;
pub use agent_contracts::*;
pub use binding::*;
pub use contracts::*;
pub use management::{ManagedCommand, ManagementConfig};
pub use routes::*;
pub use service::*;
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct Error {
    pub status: u16,
    pub message: &'static str,
}
pub type Result<T> = std::result::Result<T, Error>;
fn invalid() -> Error {
    Error {
        status: 400,
        message: "Invalid database request.",
    }
}
fn denied() -> Error {
    Error {
        status: 404,
        message: "Database access is unavailable.",
    }
}
fn unavailable() -> Error {
    Error {
        status: 503,
        message: "Database service is unavailable.",
    }
}
impl From<sea_orm::DbErr> for Error {
    fn from(error: sea_orm::DbErr) -> Self {
        let code = match &error {
            sea_orm::DbErr::Query(sea_orm::RuntimeErr::SqlxError(error))
            | sea_orm::DbErr::Exec(sea_orm::RuntimeErr::SqlxError(error)) => {
                error.as_database_error().and_then(|error| error.code())
            }
            _ => None,
        };
        match code.as_deref() {
            Some("WD051") => {
                return conflict(
                    "Backup authority requires an active scope and observed stop before archival.",
                );
            }
            Some("WD052") => {
                return conflict(
                    "Finish backup work and qualify current backup resource authority before resizing.",
                );
            }
            _ => {}
        }
        unavailable()
    }
}

fn conflict(message: &'static str) -> Error {
    Error {
        status: 409,
        message,
    }
}

/// Native callback purpose is chosen by the fixed authenticated RPC method, never caller JSON.
#[derive(Clone, Copy, Debug)]
pub enum DatabaseCallback {
    Guard,
    Checkpoint(DatabaseCheckpointPhase),
    Completion(DatabaseCompletionOutcome),
}

/// Public status keeps unqualified recovery actions disabled and measured SLOs absent.
pub const BACKUP_STATUS_SCHEMA: &str = include_str!("backup-status.schema.json");

/// Exact native managed command contract for first-party callers.
pub const MANAGED_COMMAND_SCHEMA: &str = include_str!("managed-command.schema.json");

mod export;
pub use export::DatabaseExportSubject;
