ALTER TABLE webdock_auth.plugin_invocation ADD COLUMN request_hash text CHECK(request_hash ~ '^[a-f0-9]{64}$');
CREATE INDEX plugin_invocation_operation ON webdock_auth.plugin_invocation(plugin_id,project_id,actor_id,operation_id);
