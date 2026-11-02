-- Native0032: API tokens for one workspace. Each token is its own session of the member who made it,
-- so it acts with that member's role and loses access with it; the token also limits the session to
-- one customer and, unless it may write, to reading. Only a SHA-256 of the token is stored.
CREATE TABLE webdock_auth.api_token (
 id text PRIMARY KEY CHECK (id ~ '^[1-9][0-9]{0,18}$'),
 customer_id text NOT NULL REFERENCES webdock_admin.customers(id) ON DELETE CASCADE,
 session_id text NOT NULL UNIQUE REFERENCES webdock_auth.session(id) ON DELETE CASCADE,
 user_id text NOT NULL,
 name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80),
 token_hash text NOT NULL UNIQUE CHECK (token_hash ~ '^[a-f0-9]{64}$'),
 can_write boolean NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now(),
 expires_at timestamptz NOT NULL,
 last_used_at timestamptz
);
ALTER TABLE webdock_auth.api_token OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.api_token ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.api_token FORCE ROW LEVEL SECURITY;
REVOKE ALL ON TABLE webdock_auth.api_token FROM PUBLIC;
CREATE POLICY platform_owner ON webdock_auth.api_token TO webdock_platform_owner USING (true) WITH CHECK (true);
-- Owners, admins and operators manage a workspace's tokens; they see each other's, never the token.
CREATE POLICY customer_manage ON webdock_auth.api_token TO webdock_auth_runtime
 USING (webdock_rls.customer_allowed(customer_id,true)) WITH CHECK (webdock_rls.customer_allowed(customer_id,true));
GRANT SELECT,INSERT,DELETE ON TABLE webdock_auth.api_token TO webdock_auth_runtime;

-- The session behind a bearer token, before anyone is signed in. Notes when it was last used, at most
-- once a minute.
CREATE FUNCTION webdock_rls.api_token_session(hash text) RETURNS jsonb
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE t webdock_auth.api_token;
BEGIN
 SELECT * INTO t FROM webdock_auth.api_token WHERE token_hash=hash AND expires_at>statement_timestamp();
 IF NOT FOUND THEN RETURN NULL; END IF;
 IF t.last_used_at IS NULL OR t.last_used_at<statement_timestamp()-interval '1 minute' THEN
  UPDATE webdock_auth.api_token SET last_used_at=statement_timestamp() WHERE id=t.id;
 END IF;
 RETURN jsonb_build_object('user',t.user_id,'session',t.session_id,'customer',t.customer_id,'write',t.can_write);
END $$;
ALTER FUNCTION webdock_rls.api_token_session(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.api_token_session(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.api_token_session(text) TO webdock_auth_runtime;
