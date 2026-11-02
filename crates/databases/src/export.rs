//! Confidential projection for the archives product. Native database/source authority stays in Core.
pub struct DatabaseExportSubject<'a> {
    pub customer: &'a str,
    pub project: &'a str,
    pub subject: &'a str,
    pub session: &'a str,
}
