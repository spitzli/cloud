// A claimed setup may read only its stored identity while obtaining fresh provider proof.
export const setupClerkRevalidationSQL = String.raw`
CREATE OR REPLACE FUNCTION webdock_rls.setup_clerk_revalidation(issuer text, clerk_user text, clerk_session text, clerk_org text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
 SELECT jsonb_build_object(
  'identity',jsonb_build_object('id',u.id,'name',u.name,'email',u.email,'role',u.role,'banned',u.banned,'emailVerified',u."emailVerified",'enabled',i.enabled),
  'organization',CASE WHEN o.organization_id IS NOT NULL THEN jsonb_build_object('id',o.organization_id,'customerID',s.customer_id,'active',c.status='active','enabled',o.enabled) ELSE NULL END)
 FROM webdock_auth.hosting_setup s
 JOIN webdock_admin.customers c ON c.id=s.customer_id
 JOIN webdock_admin.projects p ON p.id=s.project_id AND p.customer_id=s.customer_id
 JOIN webdock_auth.clerk_identity i ON i.issuer=s.actor->'consoleIdentity'->>'issuer' AND i.clerk_user_id=s.actor->'consoleIdentity'->>'userID' AND i.user_id=s.actor->>'subject'
 JOIN webdock_auth."user" u ON u.id=i.user_id
 JOIN webdock_auth.tenant_customer t ON t.customer_id=s.customer_id
 LEFT JOIN webdock_auth.clerk_organization o ON o.issuer=i.issuer AND o.clerk_organization_id=s.actor->'consoleIdentity'->>'organizationID' AND o.organization_id=t.organization_id
 WHERE webdock_rls.worker(ARRAY['hosting-setup-revalidation'])
 AND s.id=webdock_rls.request_scope()->>'setupID' AND s.customer_id=webdock_rls.request_scope()->>'customerID'
 AND s.generation::text=webdock_rls.request_scope()->>'generation'
 AND s.state='running' AND s.confirmed_preview_id IS NOT NULL AND s.lease_until>statement_timestamp()
 AND c.status='active' AND p.status='active'
 AND s.actor->'scopes' ? 'hosting:write' AND s.actor->'scopes' ? 'hosting:read'
 AND i.issuer=setup_clerk_revalidation.issuer AND i.clerk_user_id=clerk_user
 AND s.actor->'consoleIdentity'->>'sessionID'=clerk_session AND s.actor->>'sessionID'=clerk_session
 AND (s.actor->'consoleIdentity'->>'organizationID') IS NOT DISTINCT FROM clerk_org
 AND (clerk_org IS NULL OR o.organization_id IS NOT NULL)
 AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=s.project_id AND d.status<>'deleted')
$$;
ALTER FUNCTION webdock_rls.setup_clerk_revalidation(text,text,text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.setup_clerk_revalidation(text,text,text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.setup_clerk_revalidation(text,text,text,text) TO webdock_worker_runtime;
`;

export const setupLegacyRevalidationSQL = String.raw`
DROP FUNCTION IF EXISTS webdock_rls.setup_legacy_revalidation();
CREATE OR REPLACE FUNCTION webdock_rls.setup_legacy_revalidation(issuer text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
 SELECT jsonb_build_object('issuer',i.issuer,'userID',i.clerk_user_id,'organizationID',o.organization_id,'clerkOrganizationID',o.clerk_organization_id,'customerID',s.customer_id,'enabled',i.enabled AND coalesce(o.enabled,true))
 FROM webdock_auth.hosting_setup s
 JOIN webdock_admin.customers c ON c.id=s.customer_id AND c.status='active'
 JOIN webdock_admin.projects p ON p.id=s.project_id AND p.customer_id=s.customer_id AND p.status='active'
 JOIN webdock_auth.tenant_customer t ON t.customer_id=s.customer_id
 LEFT JOIN webdock_auth.clerk_organization o ON o.organization_id=t.organization_id AND o.issuer=setup_legacy_revalidation.issuer
 JOIN webdock_auth."user" u ON u.id=s.actor->>'subject' AND NOT coalesce(u.banned,false) AND u."emailVerified" AND NOT u."mustChangePassword"
 JOIN webdock_auth.session session ON session.id=s.actor->>'sessionID' AND session."userId"=u.id AND session."expiresAt">statement_timestamp()
 JOIN webdock_auth.clerk_identity i ON i.user_id=u.id AND i.issuer=setup_legacy_revalidation.issuer
 WHERE webdock_rls.worker(ARRAY['hosting-setup-revalidation']) AND s.actor->'consoleIdentity' IS NULL
 AND s.id=webdock_rls.request_scope()->>'setupID' AND s.customer_id=webdock_rls.request_scope()->>'customerID' AND s.generation::text=webdock_rls.request_scope()->>'generation'
 AND s.state='running' AND s.confirmed_preview_id IS NOT NULL AND s.lease_until>statement_timestamp()
 AND s.actor->'scopes' ? 'hosting:read' AND s.actor->'scopes' ? 'hosting:write'
 AND NOT EXISTS(SELECT 1 FROM webdock_auth.studio_tenant_preview preview WHERE preview.session_id=session.id)
 AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=s.project_id AND d.status<>'deleted')
$$;
CREATE OR REPLACE FUNCTION webdock_rls.setup_legacy_permission(customer text,permission text) RETURNS boolean
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE proof jsonb; org text;
BEGIN
 IF NOT webdock_rls.worker(ARRAY['hosting-setup']) THEN RETURN false; END IF;
 SELECT organization_id INTO org FROM webdock_auth.tenant_customer WHERE customer_id=customer;
 IF webdock_rls.request_scope()->>'authProvider' IS DISTINCT FROM 'clerk' THEN
  RETURN EXISTS(SELECT 1 FROM webdock_auth.hosting_setup s JOIN webdock_auth.member m ON m."userId"=s.actor->>'subject' AND m."organizationId"=org WHERE s.id=webdock_rls.request_scope()->>'setupID' AND m.role IN ('owner','admin'));
 END IF;
 proof:=webdock_rls.request_scope()->'memberships';
 IF jsonb_typeof(proof) IS DISTINCT FROM 'object' OR jsonb_typeof(proof->'operator') IS DISTINCT FROM 'boolean' OR jsonb_typeof(proof->'twoFactorEnabled') IS DISTINCT FROM 'boolean'
  OR proof->>'expiresAt' IS NULL OR proof->>'verifiedAt' IS NULL OR (proof->>'expiresAt')::numeric<=extract(epoch FROM statement_timestamp())*1000 OR (proof->>'verifiedAt')::numeric NOT BETWEEN extract(epoch FROM statement_timestamp())*1000-60000 AND extract(epoch FROM statement_timestamp())*1000 OR (proof->>'expiresAt')::numeric>(proof->>'verifiedAt')::numeric+60000 THEN RETURN false; END IF;
 IF NOT EXISTS(SELECT 1 FROM webdock_auth.hosting_setup s JOIN webdock_auth.clerk_identity i ON i.user_id=s.actor->>'subject' AND i.issuer=proof->>'issuer' AND i.clerk_user_id=proof->>'userID' AND i.enabled WHERE s.id=webdock_rls.request_scope()->>'setupID' AND proof->>'subject'=s.actor->>'subject' AND proof->>'sessionID'=s.actor->>'sessionID') THEN RETURN false; END IF;
 IF (proof->>'operator')::boolean THEN RETURN (proof->>'twoFactorEnabled')::boolean; END IF;
 RETURN EXISTS(SELECT 1 FROM webdock_auth.clerk_organization o CROSS JOIN jsonb_array_elements(proof->'roles') r WHERE o.issuer=proof->>'issuer' AND o.organization_id=org AND o.enabled AND r->>'organizationID'=org AND r->>'customerID'=customer AND r->>'role' IN ('admin','member') AND jsonb_typeof(r->'permissions')='array' AND r->'permissions' ? permission);
EXCEPTION WHEN invalid_text_representation OR numeric_value_out_of_range OR invalid_parameter_value THEN RETURN false;
END $$;
ALTER FUNCTION webdock_rls.setup_legacy_revalidation(text) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.setup_legacy_permission(text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.setup_legacy_revalidation(text),webdock_rls.setup_legacy_permission(text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.setup_legacy_revalidation(text),webdock_rls.setup_legacy_permission(text,text) TO webdock_worker_runtime,webdock_platform_owner;
`;
