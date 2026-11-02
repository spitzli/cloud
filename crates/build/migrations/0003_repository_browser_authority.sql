-- Native0016: private server-owned session/client proof for repository callbacks.
-- Never part of provider payload, public configuration or signed provider input.
ALTER TABLE webdock_auth.repository_lease ADD COLUMN IF NOT EXISTS actor_authority jsonb;
ALTER TABLE webdock_auth.repository_lease DROP CONSTRAINT IF EXISTS repository_actor_authority_shape;
ALTER TABLE webdock_auth.repository_lease ADD CONSTRAINT repository_actor_authority_shape CHECK (
 actor_authority IS NULL OR (authority_kind IN ('session','release') AND jsonb_typeof(actor_authority)='object' AND octet_length(actor_authority::text)<=16384)
);
COMMENT ON COLUMN webdock_auth.repository_lease.actor_authority IS 'Server-owned StoredAuthority; NULL retained leases use only explicitly local-session or durable git-policy authority.';
