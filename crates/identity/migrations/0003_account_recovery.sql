CREATE TABLE IF NOT EXISTS webdock_auth.identity_recovery_token (
 token_hash text PRIMARY KEY,
 user_id text NOT NULL REFERENCES webdock_auth."user"(id) ON DELETE CASCADE,
 email text NOT NULL,
 credential_digest text NOT NULL,
 kind text NOT NULL CHECK(kind IN ('reset','activation')),
 expires_at timestamptz NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS webdock_auth.identity_delivery_job (
 id text PRIMARY KEY DEFAULT webdock_auth.next_snowflake(),
 token_hash text NOT NULL REFERENCES webdock_auth.identity_recovery_token(token_hash) ON DELETE CASCADE,
 payload text NOT NULL,
 attempts integer NOT NULL DEFAULT 0 CHECK(attempts BETWEEN 0 AND 5),
 delivered_at timestamptz,
 next_attempt_at timestamptz NOT NULL DEFAULT now(),
 lease_until timestamptz,
 created_at timestamptz NOT NULL DEFAULT now()
);
DO $$ DECLARE t text; BEGIN
 FOREACH t IN ARRAY ARRAY['identity_recovery_token','identity_delivery_job'] LOOP
  EXECUTE format('REVOKE ALL ON webdock_auth.%I FROM PUBLIC',t);
  IF EXISTS(SELECT FROM pg_roles WHERE rolname='webdock_auth_bootstrap') THEN
   EXECUTE format('GRANT SELECT,INSERT,UPDATE,DELETE ON webdock_auth.%I TO webdock_auth_bootstrap',t);
  END IF;
  IF EXISTS(SELECT FROM pg_roles WHERE rolname='webdock_platform_owner') AND EXISTS(SELECT FROM pg_roles WHERE rolname='webdock_auth_bootstrap') THEN
   EXECUTE format('ALTER TABLE webdock_auth.%I OWNER TO webdock_platform_owner',t);
   EXECUTE format('ALTER TABLE webdock_auth.%I ENABLE ROW LEVEL SECURITY',t);
   EXECUTE format('ALTER TABLE webdock_auth.%I FORCE ROW LEVEL SECURITY',t);
   EXECUTE format('DROP POLICY IF EXISTS identity_owner ON webdock_auth.%I',t);
   EXECUTE format('CREATE POLICY identity_owner ON webdock_auth.%I TO webdock_platform_owner USING(true) WITH CHECK(true)',t);
   EXECUTE format('DROP POLICY IF EXISTS identity_bootstrap ON webdock_auth.%I',t);
   EXECUTE format('CREATE POLICY identity_bootstrap ON webdock_auth.%I TO webdock_auth_bootstrap USING(true) WITH CHECK(true)',t);
  END IF;
 END LOOP;
END $$;
