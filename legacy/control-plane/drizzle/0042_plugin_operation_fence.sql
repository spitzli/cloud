-- A later revision cannot replace an in-flight lifecycle operation.
CREATE UNIQUE INDEX plugin_operation_active_installation
ON webdock_auth.plugin_operation(plugin_id)
WHERE status IN ('pending','running','blocked');
