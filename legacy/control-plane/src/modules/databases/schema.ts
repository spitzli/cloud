export const databaseSchemaSQL = `
CREATE TABLE IF NOT EXISTS webdock_auth.database_binding (
 id varchar PRIMARY KEY DEFAULT webdock_auth.next_snowflake(),
 project_id varchar NOT NULL REFERENCES webdock_admin.projects(id),
 name text NOT NULL CHECK(length(name) BETWEEN 1 AND 120),
 environment text NOT NULL CHECK(environment IN ('production','staging','development')),
 enabled boolean NOT NULL DEFAULT true, created_at timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE webdock_auth.database_binding ADD COLUMN IF NOT EXISTS engine text NOT NULL DEFAULT 'postgresql' CHECK(engine IN ('postgresql','sqlite'));
CREATE TABLE IF NOT EXISTS webdock_auth.database_grant (
 id varchar PRIMARY KEY DEFAULT webdock_auth.next_snowflake(),
 binding_id varchar NOT NULL REFERENCES webdock_auth.database_binding(id),
 subject text NOT NULL REFERENCES webdock_auth."user"(id),
 profile text NOT NULL CHECK(profile IN ('read','write','schema')),
 runtime_origin text NOT NULL UNIQUE, connection_id text NOT NULL,
 secret_sealed text NOT NULL, revision integer NOT NULL DEFAULT 1,
 enabled boolean NOT NULL DEFAULT true, UNIQUE(binding_id,subject)
);
CREATE TABLE IF NOT EXISTS webdock_auth.database_launch (
 token_hash text PRIMARY KEY, grant_id varchar NOT NULL REFERENCES webdock_auth.database_grant(id),
 grant_revision integer NOT NULL, session_id text NOT NULL REFERENCES webdock_auth.session(id) ON DELETE CASCADE,
 expires_at timestamptz NOT NULL, consumed_at timestamptz
);
CREATE TABLE IF NOT EXISTS webdock_auth.database_runtime_owner (
 runtime_origin text PRIMARY KEY, binding_id varchar NOT NULL REFERENCES webdock_auth.database_binding(id),
 subject text NOT NULL REFERENCES webdock_auth."user"(id), profile text NOT NULL CHECK(profile IN ('read','write','schema'))
);
INSERT INTO webdock_auth.database_runtime_owner(runtime_origin,binding_id,subject,profile)
 SELECT runtime_origin,binding_id,subject,profile FROM webdock_auth.database_grant ON CONFLICT DO NOTHING;
CREATE TABLE IF NOT EXISTS webdock_auth.database_session (
 token_hash text PRIMARY KEY, id varchar NOT NULL UNIQUE DEFAULT webdock_auth.next_snowflake(),
 grant_id varchar NOT NULL REFERENCES webdock_auth.database_grant(id), grant_revision integer NOT NULL,
 session_id text NOT NULL REFERENCES webdock_auth.session(id) ON DELETE CASCADE, expires_at timestamptz NOT NULL
);
CREATE INDEX IF NOT EXISTS database_session_expiry ON webdock_auth.database_session(expires_at);
CREATE INDEX IF NOT EXISTS database_launch_expiry ON webdock_auth.database_launch(expires_at);
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
CREATE TABLE IF NOT EXISTS webdock_auth.database_audit (
 id varchar PRIMARY KEY DEFAULT webdock_auth.next_snowflake(), subject text NOT NULL,
 action text NOT NULL, binding_id varchar NOT NULL REFERENCES webdock_auth.database_binding(id),
 created_at timestamptz NOT NULL DEFAULT now()
);`;
