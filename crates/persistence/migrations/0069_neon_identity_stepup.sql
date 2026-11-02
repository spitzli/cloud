-- Managed Auth owns the first factor; Core owns canonical IDs, MFA, RBAC and session provenance.
-- Temporary owner-only DDL permission, revoked after function ownership is established.
GRANT CREATE ON SCHEMA webdock_rls TO webdock_platform_owner;
CREATE TABLE webdock_auth.neon_identity (
 user_id text PRIMARY KEY REFERENCES webdock_auth."user"(id) ON DELETE RESTRICT,
 provider_origin text NOT NULL CHECK(provider_origin LIKE 'https://%/auth'),
 provider_subject uuid NOT NULL UNIQUE,
 enabled boolean NOT NULL DEFAULT true,
 revision integer NOT NULL DEFAULT 1 CHECK(revision>0),
 linked_at timestamptz NOT NULL DEFAULT statement_timestamp(),
 linked_by text NOT NULL
);
CREATE TABLE webdock_auth.neon_session (
 session_id text PRIMARY KEY REFERENCES webdock_auth.session(id) ON DELETE CASCADE,
 user_id text NOT NULL REFERENCES webdock_auth.neon_identity(user_id) ON DELETE CASCADE,
 provider_session_id uuid NOT NULL,
 provider_subject uuid NOT NULL,
 identity_revision integer NOT NULL,
 credential_digest text NOT NULL CHECK(credential_digest ~ '^[0-9a-f]{64}$'),
 encrypted_cookie text NOT NULL,
 mfa_verified boolean NOT NULL DEFAULT false,
 created_at timestamptz NOT NULL DEFAULT statement_timestamp()
);
CREATE TABLE webdock_auth.neon_mfa_challenge (
 token_hash text PRIMARY KEY REFERENCES webdock_auth.identity_mfa_challenge(token_hash) ON DELETE CASCADE,
 user_id text NOT NULL REFERENCES webdock_auth.neon_identity(user_id) ON DELETE CASCADE,
 provider_session_id uuid NOT NULL,
 provider_subject uuid NOT NULL,
 identity_revision integer NOT NULL,
 credential_digest text NOT NULL CHECK(credential_digest ~ '^[0-9a-f]{64}$'),
 encrypted_cookie text NOT NULL
);
DO $$ DECLARE table_name text; BEGIN
 FOREACH table_name IN ARRAY ARRAY['neon_identity','neon_session','neon_mfa_challenge'] LOOP
  EXECUTE format('ALTER TABLE webdock_auth.%I OWNER TO webdock_platform_owner',table_name);
  EXECUTE format('REVOKE ALL ON webdock_auth.%I FROM PUBLIC',table_name);
  EXECUTE format('GRANT SELECT,INSERT,UPDATE,DELETE ON webdock_auth.%I TO webdock_auth_bootstrap',table_name);
 END LOOP;
 -- Provider tables are optional on ordinary local native installations; enable only after qualification.
 IF to_regclass('neon_auth.session') IS NOT NULL THEN
  GRANT USAGE ON SCHEMA neon_auth TO webdock_platform_owner;
  GRANT SELECT(id,"userId","expiresAt",token,"impersonatedBy") ON neon_auth.session TO webdock_platform_owner;
  GRANT SELECT(id,email,"emailVerified",banned) ON neon_auth."user" TO webdock_platform_owner;
  GRANT SELECT("userId","providerId",password) ON neon_auth.account TO webdock_platform_owner;
 END IF;
END $$;
CREATE FUNCTION webdock_rls.neon_session_facts(secret_token text) RETURNS jsonb
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,pg_temp AS $$
DECLARE facts jsonb; BEGIN
 IF secret_token IS NULL OR char_length(secret_token) NOT BETWEEN 16 AND 4096 THEN RETURN NULL; END IF;
 SELECT jsonb_build_object('sessionID',s.id,'subject',u.id,'email',u.email,'emailVerified',u."emailVerified",'expiresAt',s."expiresAt",'passwordHash',a.password)
 INTO facts FROM neon_auth.session s JOIN neon_auth."user" u ON u.id=s."userId"
 JOIN neon_auth.account a ON a."userId"=u.id AND a."providerId"='credential'
 WHERE s.token=secret_token AND s."expiresAt">statement_timestamp() AND s."impersonatedBy" IS NULL
 AND NOT coalesce(u.banned,false) AND a.password IS NOT NULL;
 RETURN facts;
END $$;
ALTER FUNCTION webdock_rls.neon_session_facts(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.neon_session_facts(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.neon_session_facts(text) TO webdock_auth_bootstrap;

CREATE FUNCTION webdock_rls.neon_binding_live(subject text,provider_session uuid,provider_user uuid,binding_revision integer,credential_hash text) RETURNS boolean
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,pg_temp AS $$
BEGIN
 RETURN EXISTS(SELECT 1 FROM webdock_auth.neon_identity n JOIN neon_auth."user" u ON u.id=n.provider_subject
 JOIN neon_auth.session s ON s."userId"=u.id AND s.id=provider_session
 JOIN neon_auth.account a ON a."userId"=u.id AND a."providerId"='credential'
 WHERE n.user_id=subject AND n.provider_subject=provider_user AND n.revision=binding_revision AND n.enabled
 AND NOT coalesce(u.banned,false) AND s."expiresAt">statement_timestamp() AND s."impersonatedBy" IS NULL
 AND credential_hash=encode(pg_catalog.sha256(pg_catalog.convert_to(a.password,'UTF8')),'hex'));
END $$;
ALTER FUNCTION webdock_rls.neon_binding_live(text,uuid,uuid,integer,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.neon_binding_live(text,uuid,uuid,integer,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.neon_binding_live(text,uuid,uuid,integer,text) TO webdock_auth_bootstrap;

CREATE FUNCTION webdock_rls.native_provider_session_live(subject text,native_session text) RETURNS boolean
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,pg_temp AS $$
DECLARE binding record; BEGIN
 SELECT * INTO binding FROM webdock_auth.neon_identity n WHERE n.user_id=subject;
 IF NOT FOUND THEN RETURN true; END IF;
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
  AND (n.mfa_verified OR EXISTS(SELECT 1 FROM webdock_auth."user" native_user WHERE native_user.id=subject AND (native_user.role<>'operator' OR NOT coalesce(native_user."twoFactorEnabled",false))))
 );
END $$;
ALTER FUNCTION webdock_rls.native_provider_session_live(text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.native_provider_session_live(text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.native_provider_session_live(text,text) TO webdock_auth_bootstrap,webdock_policy_owner;

-- Replace the original actor's body in place: policies keep its OID and cannot bypass this gate.
DO $$ DECLARE definition text; BEGIN
 definition:=pg_get_functiondef('webdock_rls.actor()'::regprocedure);
 IF position('FUNCTION webdock_rls.actor()' in definition)=0 THEN RAISE EXCEPTION 'Unexpected native actor definition'; END IF;
 EXECUTE replace(definition,'FUNCTION webdock_rls.actor()','FUNCTION webdock_rls.actor_before_neon()');
 ALTER FUNCTION webdock_rls.actor_before_neon() OWNER TO webdock_policy_owner;
 REVOKE ALL ON FUNCTION webdock_rls.actor_before_neon() FROM PUBLIC;
END $$;
CREATE OR REPLACE FUNCTION webdock_rls.actor() RETURNS jsonb
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,pg_temp AS $$
DECLARE result jsonb; BEGIN
 result:=webdock_rls.actor_before_neon();
 IF result IS NULL THEN RETURN NULL; END IF;
 IF NOT webdock_rls.native_provider_session_live(result->>'subject',result->>'sessionID') THEN RETURN NULL; END IF;
 RETURN result;
END $$;
ALTER FUNCTION webdock_rls.actor() OWNER TO webdock_policy_owner;

REVOKE CREATE ON SCHEMA webdock_rls FROM webdock_platform_owner;
