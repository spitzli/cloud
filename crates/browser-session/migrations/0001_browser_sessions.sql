-- First-party browser credentials and PKCE challenges are available only to authentication.
CREATE TABLE IF NOT EXISTS webdock_auth.identity_browser_flow (
 token_hash text PRIMARY KEY,
 app_origin text NOT NULL,
 client_id text NOT NULL,
 payload text NOT NULL,
 expires_at timestamptz NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS identity_browser_flow_expiry ON webdock_auth.identity_browser_flow(expires_at);
CREATE TABLE IF NOT EXISTS webdock_auth.identity_browser_session (
 token_hash text PRIMARY KEY,
 app_origin text NOT NULL,
 client_id text NOT NULL,
 user_id text NOT NULL REFERENCES webdock_auth."user"(id) ON DELETE CASCADE,
 session_id text NOT NULL REFERENCES webdock_auth.session(id) ON DELETE CASCADE,
 payload text NOT NULL,
 expires_at timestamptz NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS identity_browser_session_expiry ON webdock_auth.identity_browser_session(expires_at);
DO $$ DECLARE t text; BEGIN
 FOREACH t IN ARRAY ARRAY['identity_browser_flow','identity_browser_session'] LOOP
  EXECUTE format('ALTER TABLE webdock_auth.%I OWNER TO webdock_platform_owner',t);
  EXECUTE format('ALTER TABLE webdock_auth.%I ENABLE ROW LEVEL SECURITY',t);
  EXECUTE format('ALTER TABLE webdock_auth.%I FORCE ROW LEVEL SECURITY',t);
  EXECUTE format('REVOKE ALL ON webdock_auth.%I FROM PUBLIC,webdock_auth_runtime,webdock_worker_runtime,webdock_registry_runtime,webdock_auth_bootstrap',t);
  EXECUTE format('DROP POLICY IF EXISTS platform_owner ON webdock_auth.%I',t);
  EXECUTE format('CREATE POLICY platform_owner ON webdock_auth.%I TO webdock_platform_owner USING(true) WITH CHECK(true)',t);
  EXECUTE format('DROP POLICY IF EXISTS auth_bootstrap ON webdock_auth.%I',t);
  EXECUTE format('CREATE POLICY auth_bootstrap ON webdock_auth.%I TO webdock_auth_bootstrap USING(true) WITH CHECK(true)',t);
  EXECUTE format('GRANT SELECT,INSERT,UPDATE,DELETE ON webdock_auth.%I TO webdock_auth_bootstrap',t);
 END LOOP;
END $$;
