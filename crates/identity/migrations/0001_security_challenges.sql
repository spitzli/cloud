-- Additive offline migration. Apply through the platform migration journal, never at API startup.
CREATE TABLE IF NOT EXISTS webdock_auth.identity_mfa_challenge (
 token_hash text PRIMARY KEY,
 user_id text NOT NULL REFERENCES webdock_auth."user"(id) ON DELETE CASCADE,
 credential_digest text NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now(),
 expires_at timestamptz NOT NULL,
 attempts integer NOT NULL DEFAULT 0 CHECK(attempts BETWEEN 0 AND 5)
);
CREATE INDEX IF NOT EXISTS identity_mfa_challenge_user_idx ON webdock_auth.identity_mfa_challenge(user_id);
CREATE TABLE IF NOT EXISTS webdock_auth.identity_totp_replay (
 user_id text PRIMARY KEY REFERENCES webdock_auth."user"(id) ON DELETE CASCADE,
 secret_digest text NOT NULL,
 last_step bigint NOT NULL
);
CREATE TABLE IF NOT EXISTS webdock_auth.identity_mfa_enrollment (
 session_id text PRIMARY KEY REFERENCES webdock_auth.session(id) ON DELETE CASCADE,
 credential_digest text NOT NULL,
 expires_at timestamptz NOT NULL
);
REVOKE ALL ON webdock_auth.identity_mfa_challenge,webdock_auth.identity_totp_replay,webdock_auth.identity_mfa_enrollment FROM PUBLIC;
-- Authentication/bootstrap role only. Business and worker roles must not receive these grants.
DO $$ BEGIN
 IF EXISTS(SELECT FROM pg_roles WHERE rolname='webdock_auth_bootstrap') THEN
  GRANT SELECT,INSERT,UPDATE,DELETE ON webdock_auth.identity_mfa_challenge,webdock_auth.identity_totp_replay,webdock_auth.identity_mfa_enrollment TO webdock_auth_bootstrap;
 END IF;
END $$;
DO $$ DECLARE t text; BEGIN
 IF EXISTS(SELECT FROM pg_roles WHERE rolname='webdock_platform_owner')
 AND EXISTS(SELECT FROM pg_roles WHERE rolname='webdock_auth_bootstrap') THEN
  GRANT USAGE ON SCHEMA webdock_auth TO webdock_platform_owner;
  FOREACH t IN ARRAY ARRAY['identity_mfa_challenge','identity_totp_replay','identity_mfa_enrollment'] LOOP
   EXECUTE format('ALTER TABLE webdock_auth.%I OWNER TO webdock_platform_owner',t);
   EXECUTE format('ALTER TABLE webdock_auth.%I ENABLE ROW LEVEL SECURITY',t);
   EXECUTE format('ALTER TABLE webdock_auth.%I FORCE ROW LEVEL SECURITY',t);
   EXECUTE format('DROP POLICY IF EXISTS identity_owner ON webdock_auth.%I',t);
   EXECUTE format('CREATE POLICY identity_owner ON webdock_auth.%I TO webdock_platform_owner USING(true) WITH CHECK(true)',t);
   EXECUTE format('DROP POLICY IF EXISTS identity_bootstrap ON webdock_auth.%I',t);
   EXECUTE format('CREATE POLICY identity_bootstrap ON webdock_auth.%I TO webdock_auth_bootstrap USING(true) WITH CHECK(true)',t);
  END LOOP;
 END IF;
END $$;
