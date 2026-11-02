//! Narrow database-only caller for the domain-owned review regression fixture.
#[path = "../../databases/tests/support/backup_review.rs"]
mod backup_review;
#[allow(
    dead_code,
    reason = "Shared real password/TOTP identity fixture exports other helpers."
)]
#[path = "../../agent-control/tests/support/database_common.rs"]
mod support;
