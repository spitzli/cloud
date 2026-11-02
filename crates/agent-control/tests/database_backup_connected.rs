//! Narrow database-only caller for the domain-owned connected qualification fixture.
#[path = "../../databases/tests/support/connected_backup.rs"]
mod connected_backup;
#[allow(
    dead_code,
    reason = "The shared identity fixture also exports unrelated delivery-test helpers."
)]
#[path = "support/database_common.rs"]
mod support;
