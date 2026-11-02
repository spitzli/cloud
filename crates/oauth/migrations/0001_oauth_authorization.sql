-- Additive server-side consent/code state. Existing clients/tokens/consents/keys remain authoritative.
CREATE TABLE IF NOT EXISTS webdock_auth.identity_oauth_request (
 token_hash text PRIMARY KEY,
 kind text NOT NULL CHECK(kind IN ('consent','code')),
 user_id text NOT NULL REFERENCES webdock_auth."user"(id) ON DELETE CASCADE,
 session_id text NOT NULL REFERENCES webdock_auth.session(id) ON DELETE CASCADE,
 client_id text NOT NULL REFERENCES webdock_auth."oauthClient"("clientId") ON DELETE CASCADE,
 payload jsonb NOT NULL,
 expires_at timestamptz NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS identity_oauth_request_expiry ON webdock_auth.identity_oauth_request(expires_at);
REVOKE ALL ON webdock_auth.identity_oauth_request FROM PUBLIC;
DO $$ BEGIN
 IF EXISTS(SELECT FROM pg_roles WHERE rolname='webdock_auth_bootstrap') THEN
  GRANT SELECT,INSERT,UPDATE,DELETE ON webdock_auth.identity_oauth_request TO webdock_auth_bootstrap;
 END IF;
 IF EXISTS(SELECT FROM pg_roles WHERE rolname='webdock_platform_owner') AND EXISTS(SELECT FROM pg_roles WHERE rolname='webdock_auth_bootstrap') THEN
  ALTER TABLE webdock_auth.identity_oauth_request OWNER TO webdock_platform_owner;
  ALTER TABLE webdock_auth.identity_oauth_request ENABLE ROW LEVEL SECURITY;
  ALTER TABLE webdock_auth.identity_oauth_request FORCE ROW LEVEL SECURITY;
  DROP POLICY IF EXISTS identity_owner ON webdock_auth.identity_oauth_request;
  CREATE POLICY identity_owner ON webdock_auth.identity_oauth_request TO webdock_platform_owner USING(true) WITH CHECK(true);
  DROP POLICY IF EXISTS identity_bootstrap ON webdock_auth.identity_oauth_request;
  CREATE POLICY identity_bootstrap ON webdock_auth.identity_oauth_request TO webdock_auth_bootstrap USING(true) WITH CHECK(true);
 END IF;
END $$;
