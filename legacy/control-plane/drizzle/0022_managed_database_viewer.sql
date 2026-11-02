-- Apply the offline platform RLS installer after this additive migration.
CREATE TABLE IF NOT EXISTS webdock_auth.managed_database_launch (
 token_hash text PRIMARY KEY,
 database_id varchar NOT NULL REFERENCES webdock_auth.hosting_database(id),
 backend_id varchar NOT NULL REFERENCES webdock_auth.hosting_database_backend(id),
 database_revision integer NOT NULL, credential_revision integer NOT NULL,
 subject text NOT NULL REFERENCES webdock_auth."user"(id),
 session_id text NOT NULL REFERENCES webdock_auth.session(id) ON DELETE CASCADE,
 expires_at timestamptz NOT NULL, consumed_at timestamptz
);
CREATE TABLE IF NOT EXISTS webdock_auth.managed_database_session (
 token_hash text PRIMARY KEY, id varchar NOT NULL UNIQUE DEFAULT webdock_auth.next_snowflake(),
 database_id varchar NOT NULL REFERENCES webdock_auth.hosting_database(id),
 backend_id varchar NOT NULL REFERENCES webdock_auth.hosting_database_backend(id),
 database_revision integer NOT NULL, credential_revision integer NOT NULL,
 subject text NOT NULL REFERENCES webdock_auth."user"(id),
 session_id text NOT NULL REFERENCES webdock_auth.session(id) ON DELETE CASCADE,
 expires_at timestamptz NOT NULL
);
CREATE INDEX IF NOT EXISTS managed_database_launch_expiry ON webdock_auth.managed_database_launch(expires_at);
CREATE INDEX IF NOT EXISTS managed_database_session_expiry ON webdock_auth.managed_database_session(expires_at);
ALTER TABLE webdock_auth.managed_database_launch ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.managed_database_launch FORCE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.managed_database_session ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.managed_database_session FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_auth.managed_database_launch,webdock_auth.managed_database_session FROM PUBLIC;
