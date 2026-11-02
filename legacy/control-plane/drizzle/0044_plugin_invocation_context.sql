-- Retain the verified session proof for <=60s invocation capabilities across API replicas.
-- Protected by the existing forced-RLS plugin_invocation policies; never a public DTO.
ALTER TABLE webdock_auth.plugin_invocation ADD COLUMN session_context jsonb NOT NULL DEFAULT '{}'::jsonb CHECK(octet_length(session_context::text)<=16384);
