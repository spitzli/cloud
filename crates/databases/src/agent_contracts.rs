pub use webdock_agent_protocol::database::*;
impl From<DatabaseProtocolError> for crate::Error {
    fn from(_: DatabaseProtocolError) -> Self {
        crate::invalid()
    }
}
