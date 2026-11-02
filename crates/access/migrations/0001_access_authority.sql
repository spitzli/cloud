-- Offline additions. Existing identities, organizations, grants, invites and audit rows are retained.
CREATE TABLE IF NOT EXISTS webdock_auth.access_invitation_state (
 invitation_id text PRIMARY KEY REFERENCES webdock_auth.invitation(id) ON DELETE CASCADE,
 organization_id text NOT NULL REFERENCES webdock_auth.organization(id) ON DELETE CASCADE,
 user_id text NOT NULL REFERENCES webdock_auth."user"(id) ON DELETE CASCADE,
 generation integer NOT NULL DEFAULT 1 CHECK(generation>0)
);
CREATE TABLE IF NOT EXISTS webdock_auth.access_delivery_job (
 id text PRIMARY KEY,
 invitation_id text REFERENCES webdock_auth.invitation(id) ON DELETE CASCADE,
 organization_id text NOT NULL REFERENCES webdock_auth.organization(id) ON DELETE CASCADE,
 user_id text NOT NULL REFERENCES webdock_auth."user"(id) ON DELETE CASCADE,
 binding_id text REFERENCES webdock_auth.app_binding(id) ON DELETE CASCADE,
 actor_id text NOT NULL,
 actor_session_id text NOT NULL,
 generation integer NOT NULL DEFAULT 1 CHECK(generation>0),
 kind text NOT NULL CHECK(kind IN ('invitation','website-access')),
 payload text NOT NULL,
 state text NOT NULL DEFAULT 'queued' CHECK(state IN ('queued','delivered','canceled')),
 attempts integer NOT NULL DEFAULT 0 CHECK(attempts BETWEEN 0 AND 5),
 lease_until timestamptz,
 next_attempt_at timestamptz NOT NULL DEFAULT now(),
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS access_delivery_due ON webdock_auth.access_delivery_job(state,next_attempt_at);
CREATE TABLE IF NOT EXISTS webdock_auth.local_identity_activation (
 user_id text PRIMARY KEY REFERENCES webdock_auth."user"(id) ON DELETE RESTRICT,
 bindings jsonb NOT NULL,
 approved_state jsonb NOT NULL,
 evidence_ref text NOT NULL,
 evidence_sha256 text NOT NULL CHECK(evidence_sha256~'^[a-f0-9]{64}$'),
 operator_subject text NOT NULL,
 archive_envelope text NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS webdock_auth.local_organization_activation (
 organization_id text PRIMARY KEY REFERENCES webdock_auth.organization(id) ON DELETE RESTRICT,
 bindings jsonb NOT NULL,
 approved_state jsonb NOT NULL,
 evidence_ref text NOT NULL,
 evidence_sha256 text NOT NULL CHECK(evidence_sha256~'^[a-f0-9]{64}$'),
 operator_subject text NOT NULL,
 archive_envelope text NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now()
);
DO $$ DECLARE t text; BEGIN
 FOREACH t IN ARRAY ARRAY['access_invitation_state','access_delivery_job','local_identity_activation','local_organization_activation'] LOOP
  EXECUTE format('ALTER TABLE webdock_auth.%I OWNER TO webdock_platform_owner',t);
  EXECUTE format('ALTER TABLE webdock_auth.%I ENABLE ROW LEVEL SECURITY',t);
  EXECUTE format('ALTER TABLE webdock_auth.%I FORCE ROW LEVEL SECURITY',t);
  EXECUTE format('REVOKE ALL ON webdock_auth.%I FROM PUBLIC,webdock_auth_runtime,webdock_worker_runtime,webdock_registry_runtime,webdock_auth_bootstrap',t);
  EXECUTE format('DROP POLICY IF EXISTS platform_owner ON webdock_auth.%I',t);
  EXECUTE format('CREATE POLICY platform_owner ON webdock_auth.%I TO webdock_platform_owner USING(true) WITH CHECK(true)',t);
  EXECUTE format('DROP POLICY IF EXISTS auth_bootstrap ON webdock_auth.%I',t);
  EXECUTE format('CREATE POLICY auth_bootstrap ON webdock_auth.%I TO webdock_auth_bootstrap USING(true) WITH CHECK(true)',t);
 END LOOP;
END $$;
GRANT SELECT,INSERT,UPDATE,DELETE ON webdock_auth.access_invitation_state,webdock_auth.access_delivery_job TO webdock_auth_bootstrap;
GRANT SELECT(user_id,bindings) ON webdock_auth.local_identity_activation TO webdock_auth_bootstrap;
GRANT SELECT(organization_id,bindings) ON webdock_auth.local_organization_activation TO webdock_auth_bootstrap;
GRANT SELECT,INSERT,UPDATE ON webdock_auth.access_invitation_state,webdock_auth.access_delivery_job TO webdock_auth_runtime;
DROP POLICY IF EXISTS scoped_access ON webdock_auth.access_invitation_state;
CREATE POLICY scoped_access ON webdock_auth.access_invitation_state TO webdock_auth_runtime
 USING(webdock_rls.organization_allowed(organization_id,true))
 WITH CHECK(webdock_rls.organization_allowed(organization_id,true) AND EXISTS(SELECT 1 FROM webdock_auth.invitation i WHERE i.id=access_invitation_state.invitation_id AND i."organizationId"=access_invitation_state.organization_id));
DROP POLICY IF EXISTS scoped_access ON webdock_auth.access_delivery_job;
CREATE POLICY scoped_access ON webdock_auth.access_delivery_job TO webdock_auth_runtime
 USING(webdock_rls.organization_allowed(organization_id,true))
 WITH CHECK(actor_id=webdock_rls.subject() AND webdock_rls.organization_allowed(organization_id,true));

CREATE OR REPLACE FUNCTION webdock_rls.local_identity_authority(subject text) RETURNS boolean
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE actual jsonb; retired boolean; approved jsonb;
BEGIN
 SELECT jsonb_agg(jsonb_build_array(source,issuer,external_subject) ORDER BY source,issuer,external_subject),bool_and(is_retired)
 INTO actual,retired FROM (
  SELECT 'legacy'::text AS source,issuer,clerk_user_id AS external_subject,NOT enabled AS is_retired FROM webdock_auth.clerk_identity WHERE user_id=subject
  UNION ALL SELECT 'mapping',issuer,external_subject,revoked_at IS NOT NULL FROM webdock_auth.clerk_identity_mapping WHERE local_subject=subject
 ) links;
 IF actual IS NULL THEN RETURN true; END IF;
 SELECT bindings INTO approved FROM webdock_auth.local_identity_activation WHERE user_id=subject;
 RETURN coalesce(retired AND approved=actual,false);
END $$;
CREATE OR REPLACE FUNCTION webdock_rls.local_organization_authority(org text) RETURNS boolean
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE actual jsonb; retired boolean; approved jsonb;
BEGIN
 SELECT jsonb_agg(jsonb_build_array('organization',issuer,clerk_organization_id) ORDER BY issuer,clerk_organization_id),bool_and(NOT enabled)
 INTO actual,retired FROM webdock_auth.clerk_organization WHERE organization_id=org;
 IF actual IS NULL THEN RETURN true; END IF;
 SELECT bindings INTO approved FROM webdock_auth.local_organization_activation WHERE organization_id=org;
 RETURN coalesce(retired AND approved=actual,false);
END $$;
ALTER FUNCTION webdock_rls.local_identity_authority(text) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.local_organization_authority(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.local_identity_authority(text),webdock_rls.local_organization_authority(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.local_identity_authority(text),webdock_rls.local_organization_authority(text) TO webdock_auth_bootstrap,webdock_auth_runtime,webdock_registry_runtime,webdock_policy_owner;

-- Authentication may append audit events atomically with identity/invitation writes, not edit history.
CREATE OR REPLACE FUNCTION webdock_auth.append_access_event(event_id text,actor text,action_name text,target text,outcome_name text) RETURNS void
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$ BEGIN
 IF session_user<>'webdock_auth_bootstrap' OR action_name IS NULL OR length(action_name)>160 OR outcome_name NOT IN ('succeeded','failed') OR NOT EXISTS(SELECT 1 FROM webdock_auth."user" WHERE id=actor) THEN RAISE EXCEPTION 'Invalid access audit context' USING ERRCODE='42501'; END IF;
 INSERT INTO webdock_auth.access_event(id,actor_id,action,target_id,outcome) VALUES(event_id,actor,action_name,target,outcome_name);
END $$;
ALTER FUNCTION webdock_auth.append_access_event(text,text,text,text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.append_access_event(text,text,text,text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_auth.append_access_event(text,text,text,text,text) TO webdock_auth_bootstrap;

CREATE OR REPLACE FUNCTION webdock_rls.access_preview() RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
 SELECT jsonb_build_object('customerID',p.customer_id,'customerName',c.name,'organizationID',p.organization_id,'expiresAt',p.expires_at,'status',CASE WHEN p.expired_at IS NULL AND p.expires_at>statement_timestamp() AND c.status='active' AND t.organization_id=p.organization_id THEN 'active' ELSE 'expired' END,'readOnly',true,'role','admin')
 FROM webdock_auth.studio_tenant_preview p JOIN webdock_admin.customers c ON c.id=p.customer_id LEFT JOIN webdock_auth.tenant_customer t ON t.customer_id=p.customer_id
 WHERE p.session_id=webdock_rls.actor()->>'sessionID' AND p.actor_id=webdock_rls.subject() AND coalesce((webdock_rls.actor()->>'root')::boolean,false)
$$;
ALTER FUNCTION webdock_rls.access_preview() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.access_preview() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.access_preview() TO webdock_auth_runtime;

-- Native local scopes may not reuse externally projected authority during overlap.
-- Verified external-proof scopes keep their existing separate policy branches.
CREATE OR REPLACE FUNCTION webdock_rls.native_organization_authority(org text) RETURNS boolean
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE allowed boolean;
BEGIN
 IF to_regprocedure('webdock_rls.local_organization_authority(text)') IS NOT NULL THEN
  EXECUTE 'SELECT webdock_rls.local_organization_authority($1)' INTO allowed USING org;
  RETURN coalesce(allowed,false);
 END IF;
 IF to_regclass('webdock_auth.clerk_organization') IS NULL THEN RETURN true; END IF;
 EXECUTE 'SELECT NOT EXISTS(SELECT 1 FROM webdock_auth.clerk_organization WHERE organization_id=$1)' INTO allowed USING org;
 RETURN allowed;
END $$;
CREATE OR REPLACE FUNCTION webdock_rls.native_identity_authority(subject text) RETURNS boolean
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE allowed boolean;
BEGIN
 IF to_regprocedure('webdock_rls.local_identity_authority(text)') IS NOT NULL THEN
  EXECUTE 'SELECT webdock_rls.local_identity_authority($1)' INTO allowed USING subject;
  RETURN coalesce(allowed,false);
 END IF;
 IF to_regclass('webdock_auth.clerk_identity') IS NOT NULL THEN
  EXECUTE 'SELECT NOT EXISTS(SELECT 1 FROM webdock_auth.clerk_identity WHERE user_id=$1)' INTO allowed USING subject;
  IF NOT allowed THEN RETURN false; END IF;
 END IF;
 IF to_regclass('webdock_auth.clerk_identity_mapping') IS NOT NULL THEN
  EXECUTE 'SELECT NOT EXISTS(SELECT 1 FROM webdock_auth.clerk_identity_mapping WHERE local_subject=$1)' INTO allowed USING subject;
  IF NOT allowed THEN RETURN false; END IF;
 END IF;
 RETURN true;
END $$;
ALTER FUNCTION webdock_rls.native_organization_authority(text) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.native_identity_authority(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.native_organization_authority(text),webdock_rls.native_identity_authority(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.native_organization_authority(text),webdock_rls.native_identity_authority(text) TO webdock_auth_bootstrap,webdock_auth_runtime,webdock_registry_runtime,webdock_policy_owner,webdock_worker_runtime;
DO $native_guards$ DECLARE definition text; BEGIN
 SELECT pg_get_functiondef('webdock_rls.actor()'::regprocedure) INTO definition;
 IF strpos(definition,'native_identity_authority')=0 THEN
  IF strpos(definition,E' IF NOT FOUND THEN RETURN NULL; END IF;\n SELECT p.session_id')=0 THEN RAISE EXCEPTION 'Unrecognized retained actor policy'; END IF;
  EXECUTE replace(definition,E' IF NOT FOUND THEN RETURN NULL; END IF;\n SELECT p.session_id',E' IF NOT FOUND THEN RETURN NULL; END IF;\n IF NOT webdock_rls.native_identity_authority(actor_row.id) THEN RETURN NULL; END IF;\n SELECT p.session_id');
 END IF;
 SELECT pg_get_functiondef('webdock_rls.organization_allowed(text,boolean)'::regprocedure) INTO definition;
 IF strpos(definition,'native_organization_authority')=0 THEN
  IF strpos(definition,'SELECT customer_id INTO customer')=0 THEN RAISE EXCEPTION 'Unrecognized retained organization policy'; END IF;
  EXECUTE replace(definition,'SELECT customer_id INTO customer','IF NOT (a ? ''clerk'') AND coalesce(webdock_rls.request_scope()->>''authProvider'','''')<>''clerk'' AND NOT webdock_rls.native_organization_authority(org) THEN RETURN false; END IF; SELECT customer_id INTO customer');
 END IF;
 SELECT pg_get_functiondef('webdock_rls.customer_allowed(text,boolean)'::regprocedure) INTO definition;
 IF strpos(definition,'IF EXISTS(SELECT 1 FROM webdock_auth.tenant_customer WHERE customer_id=customer AND NOT webdock_rls.native_organization_authority')>0 THEN
  EXECUTE replace(definition,'IF EXISTS(SELECT 1 FROM webdock_auth.tenant_customer WHERE customer_id=customer AND NOT webdock_rls.native_organization_authority','IF NOT (a ? ''clerk'') AND coalesce(webdock_rls.request_scope()->>''authProvider'','''')<>''clerk'' AND EXISTS(SELECT 1 FROM webdock_auth.tenant_customer WHERE customer_id=customer AND NOT webdock_rls.native_organization_authority');
 ELSIF strpos(definition,'native_organization_authority')=0 THEN
  IF strpos(definition,'RETURN EXISTS(SELECT 1 FROM webdock_auth.tenant_customer')=0 THEN RAISE EXCEPTION 'Unrecognized retained customer policy'; END IF;
  EXECUTE replace(definition,'RETURN EXISTS(SELECT 1 FROM webdock_auth.tenant_customer','IF NOT (a ? ''clerk'') AND coalesce(webdock_rls.request_scope()->>''authProvider'','''')<>''clerk'' AND EXISTS(SELECT 1 FROM webdock_auth.tenant_customer WHERE customer_id=customer AND NOT webdock_rls.native_organization_authority(organization_id)) THEN RETURN false; END IF; RETURN EXISTS(SELECT 1 FROM webdock_auth.tenant_customer');
 END IF;
 IF to_regprocedure('webdock_rls.membership_role(text)') IS NOT NULL THEN
  SELECT pg_get_functiondef('webdock_rls.membership_role(text)'::regprocedure) INTO definition;
  IF strpos(definition,'native_organization_authority')=0 THEN
   IF strpos(definition,'SELECT role INTO result FROM webdock_auth.member')=0 THEN RAISE EXCEPTION 'Unrecognized retained membership policy'; END IF;
   EXECUTE replace(definition,'SELECT role INTO result FROM webdock_auth.member','IF NOT webdock_rls.native_organization_authority(org) THEN RETURN NULL; END IF; SELECT role INTO result FROM webdock_auth.member');
  END IF;
 END IF;
END $native_guards$;
