-- Project-scoped Neon API keys authorize an exact project GET rather than account inventory.
ALTER TABLE webdock_auth.neon_connection
 ADD COLUMN neon_project_id text
 CHECK(neon_project_id ~ '^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$');
