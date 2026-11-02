-- Authentication needs only the existence of an exact session's preview, not tenant data.
CREATE SCHEMA IF NOT EXISTS webdock_rls;
CREATE OR REPLACE FUNCTION webdock_rls.identity_has_preview(subject text,session_id text)
RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
 SELECT EXISTS(SELECT 1 FROM webdock_auth.studio_tenant_preview p
 WHERE p.actor_id=$1 AND p.session_id=$2)
$$;
REVOKE ALL ON FUNCTION webdock_rls.identity_has_preview(text,text) FROM PUBLIC;
DO $$ BEGIN
 IF EXISTS(SELECT FROM pg_roles WHERE rolname='webdock_policy_owner') THEN
  GRANT USAGE ON SCHEMA webdock_auth,webdock_rls TO webdock_policy_owner;
  GRANT SELECT(actor_id,session_id) ON webdock_auth.studio_tenant_preview TO webdock_policy_owner;
  ALTER FUNCTION webdock_rls.identity_has_preview(text,text) OWNER TO webdock_policy_owner;
 END IF;
 IF EXISTS(SELECT FROM pg_roles WHERE rolname='webdock_auth_bootstrap') THEN
  GRANT USAGE ON SCHEMA webdock_rls TO webdock_auth_bootstrap;
  GRANT EXECUTE ON FUNCTION webdock_rls.identity_has_preview(text,text) TO webdock_auth_bootstrap;
 END IF;
END $$;
