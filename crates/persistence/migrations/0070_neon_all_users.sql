-- Owner-controlled all-user enforcement, initially off for the reviewed cutover.
GRANT CREATE ON SCHEMA webdock_rls TO webdock_platform_owner;
CREATE TABLE webdock_auth.neon_auth_policy (
 singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
 require_managed boolean NOT NULL DEFAULT false,
 activated_by text REFERENCES webdock_auth."user"(id) ON DELETE RESTRICT,
 activated_at timestamptz,
 CHECK(NOT require_managed OR (activated_by IS NOT NULL AND activated_at IS NOT NULL))
);
ALTER TABLE webdock_auth.neon_auth_policy OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.neon_auth_policy ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.neon_auth_policy FORCE ROW LEVEL SECURITY;
CREATE POLICY platform_owner ON webdock_auth.neon_auth_policy TO webdock_platform_owner USING(true) WITH CHECK(true);
REVOKE ALL ON webdock_auth.neon_auth_policy FROM PUBLIC,webdock_auth_bootstrap,webdock_auth_runtime,webdock_registry_runtime,webdock_worker_runtime;
INSERT INTO webdock_auth.neon_auth_policy(singleton) VALUES(true);
CREATE FUNCTION webdock_rls.neon_required() RETURNS boolean
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,pg_temp AS $$
 SELECT coalesce((SELECT require_managed FROM webdock_auth.neon_auth_policy WHERE singleton),true)
$$;
ALTER FUNCTION webdock_rls.neon_required() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.neon_required() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.neon_required() TO webdock_auth_bootstrap,webdock_policy_owner;

-- Runtime may create a proved binding, never rebind or delete one to regain native login.
REVOKE UPDATE,DELETE ON webdock_auth.neon_identity FROM webdock_auth_bootstrap;
CREATE FUNCTION webdock_rls.neon_identity_immutable() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,pg_temp AS $$ BEGIN
 IF ROW(NEW.user_id,NEW.provider_origin,NEW.provider_subject,NEW.linked_at,NEW.linked_by)
    IS DISTINCT FROM ROW(OLD.user_id,OLD.provider_origin,OLD.provider_subject,OLD.linked_at,OLD.linked_by) THEN
  RAISE EXCEPTION 'Managed identity binding is immutable' USING ERRCODE='23514';
 END IF;
 IF NEW.revision<OLD.revision OR (NEW.enabled IS DISTINCT FROM OLD.enabled AND NEW.revision::bigint<>OLD.revision::bigint+1) THEN
  RAISE EXCEPTION 'Managed identity change requires a new revision' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_rls.neon_identity_immutable() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.neon_identity_immutable() FROM PUBLIC;
CREATE TRIGGER neon_identity_immutable BEFORE UPDATE ON webdock_auth.neon_identity
FOR EACH ROW EXECUTE FUNCTION webdock_rls.neon_identity_immutable();

CREATE OR REPLACE FUNCTION webdock_rls.native_provider_session_live(subject text,native_session text) RETURNS boolean
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,pg_temp AS $$
DECLARE binding record; BEGIN
 SELECT * INTO binding FROM webdock_auth.neon_identity n WHERE n.user_id=subject;
 IF NOT FOUND THEN
  -- Scoped automation remains independent; it never authenticates a browser/MFA flow.
  IF EXISTS(SELECT 1 FROM webdock_auth.api_token t WHERE t.session_id=native_session AND t.user_id=subject AND t.expires_at>statement_timestamp()) THEN RETURN true; END IF;
  RETURN NOT webdock_rls.neon_required();
 END IF;
 IF NOT binding.enabled THEN RETURN false; END IF;
 IF NOT EXISTS(SELECT 1 FROM neon_auth."user" u WHERE u.id=binding.provider_subject AND NOT coalesce(u.banned,false)) THEN RETURN false; END IF;
 -- These opaque sessions are reached only by the existing workspace-scoped bearer-token API.
 -- Their random native cookie token is never returned; they never satisfy interactive operator MFA.
 IF EXISTS(SELECT 1 FROM webdock_auth.api_token t WHERE t.session_id=native_session AND t.user_id=subject AND t.expires_at>statement_timestamp()) THEN RETURN true; END IF;
 RETURN EXISTS(
  SELECT 1 FROM webdock_auth.neon_session n JOIN neon_auth.session s ON s.id=n.provider_session_id AND s."userId"=n.provider_subject
  JOIN neon_auth.account a ON a."userId"=n.provider_subject AND a."providerId"='credential'
  WHERE n.session_id=native_session AND n.user_id=subject AND n.provider_subject=binding.provider_subject
  AND n.identity_revision=binding.revision AND s."expiresAt">statement_timestamp() AND s."impersonatedBy" IS NULL
  AND n.credential_digest=encode(pg_catalog.sha256(pg_catalog.convert_to(a.password,'UTF8')),'hex')
  AND (n.mfa_verified OR EXISTS(SELECT 1 FROM webdock_auth."user" native_user WHERE native_user.id=subject AND NOT coalesce(native_user."twoFactorEnabled",false)))
 );
END $$;
REVOKE CREATE ON SCHEMA webdock_rls FROM webdock_platform_owner;
