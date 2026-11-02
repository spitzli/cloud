-- Fail migration on ambiguous existing credential ownership; never merge credential records.
CREATE UNIQUE INDEX IF NOT EXISTS identity_passkey_credential_unique ON webdock_auth.passkey("credentialID");
CREATE TABLE IF NOT EXISTS webdock_auth.identity_passkey_ceremony (
 token_hash text PRIMARY KEY,
 kind text NOT NULL CHECK(kind IN ('registration','authentication')),
 user_id text REFERENCES webdock_auth."user"(id) ON DELETE CASCADE,
 session_id text REFERENCES webdock_auth.session(id) ON DELETE CASCADE,
 state jsonb NOT NULL,
 expires_at timestamptz NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS identity_passkey_ceremony_expiry ON webdock_auth.identity_passkey_ceremony(expires_at);
REVOKE ALL ON webdock_auth.identity_passkey_ceremony FROM PUBLIC;
DO $$ BEGIN
 IF EXISTS(SELECT FROM pg_roles WHERE rolname='webdock_auth_bootstrap') THEN
  GRANT SELECT,INSERT,UPDATE,DELETE ON webdock_auth.identity_passkey_ceremony TO webdock_auth_bootstrap;
 END IF;
 IF EXISTS(SELECT FROM pg_roles WHERE rolname='webdock_platform_owner') AND EXISTS(SELECT FROM pg_roles WHERE rolname='webdock_auth_bootstrap') THEN
  ALTER TABLE webdock_auth.identity_passkey_ceremony OWNER TO webdock_platform_owner;
  ALTER TABLE webdock_auth.identity_passkey_ceremony ENABLE ROW LEVEL SECURITY;
  ALTER TABLE webdock_auth.identity_passkey_ceremony FORCE ROW LEVEL SECURITY;
  DROP POLICY IF EXISTS identity_owner ON webdock_auth.identity_passkey_ceremony;
  CREATE POLICY identity_owner ON webdock_auth.identity_passkey_ceremony TO webdock_platform_owner USING(true) WITH CHECK(true);
  DROP POLICY IF EXISTS identity_bootstrap ON webdock_auth.identity_passkey_ceremony;
  CREATE POLICY identity_bootstrap ON webdock_auth.identity_passkey_ceremony TO webdock_auth_bootstrap USING(true) WITH CHECK(true);
 END IF;
END $$;
