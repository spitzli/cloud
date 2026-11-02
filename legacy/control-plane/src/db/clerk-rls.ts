// Inserted into actor() by the offline policy installer after Clerk schema migration.
export const clerkActorSQL = `
 IF s ? 'clerk' THEN
  IF jsonb_typeof(s->'clerk') IS DISTINCT FROM 'object' OR jsonb_typeof(s->'clerk'->'operator') IS DISTINCT FROM 'boolean' OR (s->'clerk'->>'expiresAt')::numeric<=extract(epoch FROM statement_timestamp())*1000
   OR (s->'clerk'->>'verifiedAt')::numeric>extract(epoch FROM statement_timestamp())*1000
   OR (s->'clerk'->>'verifiedAt')::numeric<extract(epoch FROM statement_timestamp())*1000-60000
   OR (s->'clerk'->>'expiresAt')::numeric>(s->'clerk'->>'verifiedAt')::numeric+60000
   OR s->'clerk'->>'expiresAt' IS NULL OR s->'clerk'->>'verifiedAt' IS NULL THEN RETURN NULL; END IF;
  SELECT u.id,u.email INTO actor_row FROM webdock_auth.clerk_identity i JOIN webdock_auth."user" u ON u.id=i.user_id
   WHERE i.issuer=s->'clerk'->>'issuer' AND i.clerk_user_id=s->'clerk'->>'userID' AND i.enabled AND u.id=s->>'subject'
   AND NOT coalesce(u.banned,false);
  IF NOT FOUND THEN RETURN NULL; END IF;
  IF s->'clerk'->>'organizationID' IS NOT NULL AND NOT EXISTS(
   SELECT 1 FROM webdock_auth.clerk_organization o JOIN webdock_auth.tenant_customer t ON t.organization_id=o.organization_id JOIN webdock_admin.customers c ON c.id=t.customer_id
   WHERE o.issuer=s->'clerk'->>'issuer' AND o.clerk_organization_id=s->'clerk'->>'clerkOrganizationID' AND o.organization_id=s->'clerk'->>'organizationID'
   AND t.customer_id=s->'clerk'->>'customerID' AND o.enabled AND c.status='active' AND s->'clerk'->>'role' IN ('admin','member')) THEN RETURN NULL; END IF;
  SELECT p.session_id,p.actor_id,p.customer_id,p.organization_id,p.expires_at,p.expired_at,
   (p.expired_at IS NULL AND p.expires_at>statement_timestamp() AND c.status='active' AND t.organization_id=p.organization_id) AS valid INTO preview_row
   FROM webdock_auth.console_tenant_preview p LEFT JOIN webdock_admin.customers c ON c.id=p.customer_id LEFT JOIN webdock_auth.tenant_customer t ON t.customer_id=p.customer_id
   WHERE p.issuer=s->'clerk'->>'issuer' AND p.session_id=s->>'sessionID' AND p.actor_id=actor_row.id;
  RETURN jsonb_build_object('subject',actor_row.id,'email',actor_row.email,'sessionID',s->>'sessionID','operator',(s->'clerk'->>'operator')::boolean AND NOT FOUND,'root',(s->'clerk'->>'operator')::boolean,'clerk',s->'clerk','preview',CASE WHEN FOUND THEN to_jsonb(preview_row) ELSE NULL END);
 END IF;
`;
export const clerkLegacyActorSQL = `
 IF s->>'authProvider'='clerk' THEN
  IF jsonb_typeof(s->'memberships') IS DISTINCT FROM 'object'
   OR jsonb_typeof(s->'memberships'->'operator') IS DISTINCT FROM 'boolean'
   OR jsonb_typeof(s->'memberships'->'twoFactorEnabled') IS DISTINCT FROM 'boolean'
   OR s->'memberships'->>'subject' IS DISTINCT FROM s->>'subject' OR s->'memberships'->>'sessionID' IS DISTINCT FROM s->>'sessionID'
   OR s->'memberships'->>'expiresAt' IS NULL OR s->'memberships'->>'verifiedAt' IS NULL
   OR (s->'memberships'->>'expiresAt')::numeric<=extract(epoch FROM statement_timestamp())*1000
   OR (s->'memberships'->>'verifiedAt')::numeric NOT BETWEEN extract(epoch FROM statement_timestamp())*1000-60000 AND extract(epoch FROM statement_timestamp())*1000
   OR (s->'memberships'->>'expiresAt')::numeric>(s->'memberships'->>'verifiedAt')::numeric+60000
   OR ((s->'memberships'->>'operator')::boolean AND NOT (s->'memberships'->>'twoFactorEnabled')::boolean) THEN RETURN NULL; END IF;
  SELECT u.id,u.email INTO actor_row FROM webdock_auth.clerk_identity i JOIN webdock_auth."user" u ON u.id=i.user_id JOIN webdock_auth.session session ON session."userId"=u.id
   WHERE i.issuer=s->'memberships'->>'issuer' AND i.clerk_user_id=s->'memberships'->>'userID' AND i.enabled AND u.id=s->>'subject'
   AND session.id=s->>'sessionID' AND session."expiresAt">statement_timestamp() AND NOT coalesce(u.banned,false) AND u."emailVerified" AND NOT u."mustChangePassword";
  IF NOT FOUND THEN RETURN NULL; END IF;
  SELECT p.session_id,p.actor_id,p.customer_id,p.organization_id,p.expires_at,p.expired_at, (p.expired_at IS NULL AND p.expires_at>statement_timestamp() AND c.status='active' AND t.organization_id=p.organization_id) AS valid INTO preview_row FROM webdock_auth.studio_tenant_preview p LEFT JOIN webdock_admin.customers c ON c.id=p.customer_id LEFT JOIN webdock_auth.tenant_customer t ON t.customer_id=p.customer_id WHERE p.session_id=s->>'sessionID' AND p.actor_id=actor_row.id;
  RETURN jsonb_build_object('subject',actor_row.id,'email',actor_row.email,'sessionID',s->>'sessionID','operator',(s->'memberships'->>'operator')::boolean AND NOT FOUND,'root',(s->'memberships'->>'operator')::boolean,'preview',CASE WHEN FOUND THEN to_jsonb(preview_row) ELSE NULL END);
 END IF;
`;
export const clerkCustomerSQL = `
 IF a ? 'clerk' THEN
  RETURN a->'clerk'->>'customerID'=customer AND a->'clerk'->>'role' IN ('admin','member') AND (NOT writing OR a->'clerk'->>'role'='admin');
 END IF;
`;

export const clerkSetupSQL = `
 IF webdock_rls.request_scope() ? 'clerk' THEN
  RETURN EXISTS(SELECT 1 FROM webdock_auth.hosting_setup s
   JOIN webdock_admin.projects p ON p.id=s.project_id JOIN webdock_admin.customers c ON c.id=s.customer_id
   JOIN webdock_auth.hosting_project h ON h.project_id=p.id JOIN webdock_auth.tenant_customer t ON t.customer_id=c.id
   JOIN webdock_auth.clerk_identity i ON i.user_id=s.actor->>'subject' AND i.issuer=s.actor->'consoleIdentity'->>'issuer' AND i.clerk_user_id=s.actor->'consoleIdentity'->>'userID'
   JOIN webdock_auth."user" u ON u.id=i.user_id
   WHERE s.id=webdock_rls.request_scope()->>'setupID' AND s.generation::text=webdock_rls.request_scope()->>'generation'
   AND s.customer_id=customer AND (project IS NULL OR s.project_id=project) AND s.customer_id=webdock_rls.request_scope()->>'customerID'
   AND s.state='running' AND s.confirmed_preview_id IS NOT NULL AND s.lease_until>now() AND c.status='active' AND p.status='active'
   AND s.actor->'scopes' ? 'hosting:write' AND s.actor->'scopes' ? 'hosting:read' AND i.enabled AND NOT coalesce(u.banned,false)
   AND webdock_rls.request_scope()->'clerk'->>'subject'=u.id AND webdock_rls.request_scope()->'clerk'->>'sessionID'=s.actor->>'sessionID'
   AND webdock_rls.request_scope()->'clerk'->>'issuer'=i.issuer AND webdock_rls.request_scope()->'clerk'->>'userID'=i.clerk_user_id
   AND (webdock_rls.request_scope()->'clerk'->>'expiresAt')::numeric>extract(epoch FROM statement_timestamp())*1000
   AND (webdock_rls.request_scope()->'clerk'->>'verifiedAt')::numeric BETWEEN extract(epoch FROM statement_timestamp())*1000-60000 AND extract(epoch FROM statement_timestamp())*1000
   AND (webdock_rls.request_scope()->'clerk'->>'expiresAt')::numeric<=(webdock_rls.request_scope()->'clerk'->>'verifiedAt')::numeric+60000
   AND jsonb_typeof(webdock_rls.request_scope()->'clerk'->'operator')='boolean'
   AND ((webdock_rls.request_scope()->'clerk'->>'operator')::boolean OR (h.mode='selfservice' AND webdock_rls.request_scope()->'clerk'->>'role' IN ('admin','member') AND jsonb_typeof(webdock_rls.request_scope()->'clerk'->'permissions')='array' AND webdock_rls.request_scope()->'clerk'->'permissions' ? 'org:projects:manage'
    AND (coalesce(s.configuration->'database'->>'mode','none')='none' OR webdock_rls.request_scope()->'clerk'->'permissions' ? 'org:databases:manage')
    AND (coalesce(s.configuration->'source'->>'kind','template')<>'git' OR webdock_rls.request_scope()->'clerk'->'permissions' ? 'org:deployments:create') AND webdock_rls.request_scope()->'clerk'->>'customerID'=customer
    AND EXISTS(SELECT 1 FROM webdock_auth.clerk_organization o WHERE o.issuer=i.issuer AND o.organization_id=t.organization_id AND o.organization_id=webdock_rls.request_scope()->'clerk'->>'organizationID' AND o.enabled AND o.clerk_organization_id=webdock_rls.request_scope()->'clerk'->>'clerkOrganizationID')))
   AND NOT EXISTS(SELECT 1 FROM webdock_auth.console_tenant_preview preview WHERE preview.issuer=i.issuer AND preview.session_id=s.actor->>'sessionID')
   AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion deletion WHERE deletion.project_id=s.project_id AND deletion.status<>'deleted'));
 END IF;
`;
export const clerkPermissionSQL = `
CREATE OR REPLACE FUNCTION webdock_rls.customer_permission(customer text, permission text, allow_operator boolean DEFAULT true) RETURNS boolean LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE a jsonb; org text; permissions jsonb;
BEGIN
 IF permission NOT IN ('org:projects:manage','org:deployments:create','org:databases:manage','org:billing:manage') THEN RETURN false; END IF;
 a:=webdock_rls.actor(); IF a IS NULL OR a->'preview'<>'null'::jsonb THEN RETURN false; END IF;
 IF allow_operator AND (a->>'operator')::boolean THEN RETURN true; END IF;
 SELECT t.organization_id INTO org FROM webdock_auth.tenant_customer t JOIN webdock_admin.customers c ON c.id=t.customer_id WHERE t.customer_id=customer AND c.status='active';
 IF NOT FOUND OR webdock_rls.membership_role(org) IS NULL THEN RETURN false; END IF;
 IF a ? 'clerk' THEN
  IF a->'clerk'->>'customerID' IS DISTINCT FROM customer OR a->'clerk'->>'organizationID' IS DISTINCT FROM org THEN RETURN false; END IF;
  permissions:=a->'clerk'->'permissions';
 ELSIF webdock_rls.request_scope()->>'authProvider'='clerk' THEN
  SELECT r->'permissions' INTO permissions FROM jsonb_array_elements(webdock_rls.request_scope()->'memberships'->'roles') r WHERE r->>'organizationID'=org AND r->>'customerID'=customer;
 ELSE RETURN webdock_rls.membership_role(org) IN ('owner','admin');
 END IF;
 IF jsonb_typeof(permissions) IS DISTINCT FROM 'array' THEN RETURN false; END IF;
 IF EXISTS(SELECT 1 FROM jsonb_array_elements(permissions) p WHERE jsonb_typeof(p)<>'string' OR p#>>'{}' NOT IN ('org:projects:manage','org:deployments:create','org:databases:manage','org:billing:manage')) THEN RETURN false; END IF;
 RETURN permissions ? permission;
EXCEPTION WHEN invalid_text_representation OR numeric_value_out_of_range OR invalid_parameter_value THEN RETURN false;
END $$;
ALTER FUNCTION webdock_rls.customer_permission(text,text,boolean) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.customer_permission(text,text,boolean) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.customer_permission(text,text,boolean) TO webdock_auth_runtime,webdock_worker_runtime,webdock_registry_runtime,webdock_policy_owner;
`;
export const clerkBillingSQL = `
CREATE OR REPLACE FUNCTION webdock_rls.billing_admin(customer text) RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
 SELECT webdock_rls.customer_permission(customer,'org:billing:manage',false)
$$;
ALTER FUNCTION webdock_rls.billing_admin(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.billing_admin(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.billing_admin(text) TO webdock_auth_runtime;
-- The retained allocation lock is also consulted when accepting a plan offer.
DO $billing_lock$ DECLARE definition text;
BEGIN
 IF to_regprocedure('webdock_rls.billing_base_locked(text)') IS NOT NULL THEN
  SELECT pg_get_functiondef(to_regprocedure('webdock_rls.billing_base_locked(text)')) INTO definition;
  EXECUTE replace(definition,'webdock_rls.customer_allowed(customer,true)','webdock_rls.customer_permission(customer,''org:billing:manage'')');
 END IF;
END $billing_lock$;
`;
export const clerkBillingReadSQL = `
CREATE OR REPLACE FUNCTION webdock_rls.billing_reader(customer text) RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
 SELECT webdock_rls.billing_admin(customer) OR coalesce(webdock_rls.actor() IS NOT NULL AND webdock_rls.actor()->'preview'='null'::jsonb AND EXISTS(SELECT 1 FROM webdock_auth.tenant_customer t JOIN webdock_admin.customers c ON c.id=t.customer_id WHERE t.customer_id=customer AND c.status='active' AND webdock_rls.membership_role(t.organization_id) IN ('owner','admin')),false)
$$;
ALTER FUNCTION webdock_rls.billing_reader(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.billing_reader(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.billing_reader(text) TO webdock_auth_runtime;
-- These functions only select purchase/catalog information; checkout mutations keep billing_admin.
DO $readers$ DECLARE signature text; definition text;
BEGIN
 FOREACH signature IN ARRAY ARRAY['webdock_rls.billing_sale_config(text,text)','webdock_rls.billing_version(text,text,text,text)','webdock_rls.billing_site(text,text,text,text)','webdock_rls.billing_catalog(text,text)','webdock_rls.billing_sites(text,text)'] LOOP
  IF to_regprocedure(signature) IS NOT NULL THEN
   SELECT pg_get_functiondef(to_regprocedure(signature)) INTO definition;
   EXECUTE replace(definition,'webdock_rls.billing_admin(customer)','webdock_rls.billing_reader(customer)');
  END IF;
 END LOOP;
END $readers$;
`;
export const clerkMembershipSQL = `
CREATE OR REPLACE FUNCTION webdock_rls.membership_role(org text) RETURNS text LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE a jsonb; proof jsonb; result text;
BEGIN
 a:=webdock_rls.actor(); IF a IS NULL THEN RETURN NULL; END IF;
 IF a ? 'clerk' THEN RETURN CASE WHEN a->'clerk'->>'organizationID'=org THEN a->'clerk'->>'role' ELSE NULL END; END IF;
 IF webdock_rls.request_scope()->>'authProvider'='clerk' THEN
  proof:=webdock_rls.request_scope()->'memberships';
  IF proof IS NULL OR proof->>'expiresAt' IS NULL OR proof->>'verifiedAt' IS NULL OR (proof->>'expiresAt')::numeric<=extract(epoch FROM statement_timestamp())*1000
   OR (proof->>'verifiedAt')::numeric NOT BETWEEN extract(epoch FROM statement_timestamp())*1000-60000 AND extract(epoch FROM statement_timestamp())*1000
   OR (proof->>'expiresAt')::numeric>(proof->>'verifiedAt')::numeric+60000 THEN RETURN NULL; END IF;
  IF NOT EXISTS(SELECT 1 FROM webdock_auth.clerk_identity i JOIN webdock_auth.clerk_organization o ON o.issuer=i.issuer WHERE i.user_id=a->>'subject' AND i.issuer=proof->>'issuer' AND i.clerk_user_id=proof->>'userID' AND i.enabled AND o.enabled AND o.organization_id=org) THEN RETURN NULL; END IF;
  SELECT r->>'role' INTO result FROM jsonb_array_elements(proof->'roles') r JOIN webdock_auth.tenant_customer t ON t.organization_id=r->>'organizationID' AND t.customer_id=r->>'customerID' WHERE r->>'organizationID'=org AND r->>'role' IN ('admin','member');
  RETURN result;
 END IF;
 IF NOT webdock_rls.native_organization_authority(org) THEN RETURN NULL; END IF;
 SELECT role INTO result FROM webdock_auth.member WHERE "organizationId"=org AND "userId"=a->>'subject';
 RETURN result;
EXCEPTION WHEN invalid_text_representation OR numeric_value_out_of_range OR invalid_parameter_value THEN RETURN NULL;
END $$;
ALTER FUNCTION webdock_rls.membership_role(text) OWNER TO webdock_policy_owner;
REVOKE ALL ON FUNCTION webdock_rls.membership_role(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.membership_role(text) TO webdock_auth_runtime,webdock_worker_runtime,webdock_registry_runtime,webdock_platform_owner;
`;
export const clerkProjectionSQL = `
CREATE OR REPLACE FUNCTION webdock_rls.project_clerk_user(issuer text,provider_user text,provider_org text,display_name text,address text) RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE local_id text;
BEGIN
 IF length(issuer)>300 OR length(provider_user)>128 OR length(display_name)>160 OR length(address)>254 OR length(address)<3 THEN RAISE EXCEPTION 'Invalid identity projection' USING ERRCODE='22023'; END IF;
 PERFORM pg_advisory_xact_lock(hashtextextended('clerk-user:'||issuer||':'||provider_user,0));
 SELECT user_id INTO local_id FROM webdock_auth.clerk_identity i WHERE i.issuer=project_clerk_user.issuer AND i.clerk_user_id=provider_user;
 IF FOUND THEN RETURN local_id; END IF;
 IF NOT EXISTS(SELECT 1 FROM webdock_auth.clerk_organization o JOIN webdock_auth.tenant_customer t ON t.organization_id=o.organization_id JOIN webdock_admin.customers c ON c.id=t.customer_id WHERE o.issuer=project_clerk_user.issuer AND o.clerk_organization_id=provider_org AND o.enabled AND c.status='active') THEN RAISE EXCEPTION 'Mapped active team required' USING ERRCODE='42501'; END IF;
 PERFORM pg_advisory_xact_lock(hashtextextended('clerk-email:'||lower(address),0));
 IF EXISTS(SELECT 1 FROM webdock_auth."user" WHERE lower(email)=lower(address)) THEN RAISE EXCEPTION 'Explicit identity mapping required' USING ERRCODE='23505'; END IF;
 INSERT INTO webdock_auth."user"(name,email,"emailVerified",role,"mustChangePassword","twoFactorEnabled") VALUES(display_name,address,true,'user',false,false) RETURNING id INTO local_id;
 INSERT INTO webdock_auth.clerk_identity(issuer,clerk_user_id,user_id,legacy_eligible) VALUES(project_clerk_user.issuer,provider_user,local_id,false);
 RETURN local_id;
END $$;
ALTER FUNCTION webdock_rls.project_clerk_user(text,text,text,text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.project_clerk_user(text,text,text,text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.project_clerk_user(text,text,text,text,text) TO webdock_auth_bootstrap;
`;
