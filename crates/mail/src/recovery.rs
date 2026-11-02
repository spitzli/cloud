//! Canonical send/incoming paths share the installation recovery epoch.
//! This never duplicates message state or grants a transport attempt.
use crate::{
    Error, Result,
    outbox::{field, one},
};
use sea_orm::DatabaseTransaction;
pub(crate) async fn ensure_open(tx: &DatabaseTransaction) -> Result<()> {
    let row = one(
        tx,
        "SELECT webdock_rls.mail_recovery_open() AS allowed",
        vec![],
    )
    .await?
    .ok_or(Error::Unavailable)?;
    if !field::<bool>(&row, "allowed")? {
        return Err(Error::Unavailable);
    }
    Ok(())
}
