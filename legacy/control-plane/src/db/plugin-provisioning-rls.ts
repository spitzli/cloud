export const pluginProvisioningRlsSQL = `
CREATE OR REPLACE FUNCTION webdock_rls.plugin_provisioning_scope(plugin text,customer text,project text,generation integer,revision integer) RETURNS boolean
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
 SELECT EXISTS(SELECT 1 FROM webdock_auth.plugin_installation p
 JOIN webdock_auth.plugin_runtime r ON r.plugin_id=p.id AND r.generation=p.current_generation
 JOIN webdock_auth.plugin_configuration c ON c.plugin_id=p.id
 JOIN webdock_auth.plugin_grant g ON g.plugin_id=p.id
 JOIN webdock_admin.projects pr ON pr.id=g.project_id AND pr.customer_id=g.customer_id AND pr.status='active'
 JOIN webdock_admin.customers cu ON cu.id=g.customer_id AND cu.status='active'
 WHERE p.id=$1 AND p.state='enabled' AND p.desired_state='enabled' AND p.runtime='ready' AND r.status='ready'
 AND p.current_generation=$4 AND r.configuration_revision=c.revision
 AND g.project_id=$3 AND g.customer_id=$2 AND g.revision=$5 AND g.revoked_at IS NULL
 AND c.approved_capabilities ? 'plugin.apps.provision' AND g.capabilities ? 'plugin.apps.provision'
 AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=$3 AND d.status<>'deleted')
 AND (webdock_rls.plugin_allows($1,$2,$3,'plugin.apps.provision')
 OR webdock_rls.plugin_allows($1,$2,$3,'plugin.apps.read')
 OR EXISTS(SELECT 1 FROM webdock_auth.hosting_plugin_provisioning a WHERE a.plugin_id=$1 AND a.project_id=$3 AND a.customer_id=$2 AND a.plugin_generation=$4 AND a.grant_revision=$5
  AND ((webdock_rls.worker(ARRAY['hosting-setup']) AND a.setup_id=webdock_rls.request_scope()->>'setupID' AND webdock_rls.setup_allowed($2,$3))
   OR webdock_rls.agent_project($3)))))
$$;
CREATE OR REPLACE FUNCTION webdock_rls.plugin_provisioning_live(provisioning_id text) RETURNS boolean
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
 SELECT EXISTS(SELECT 1 FROM webdock_auth.hosting_plugin_provisioning a WHERE a.id=provisioning_id AND webdock_rls.plugin_provisioning_scope(a.plugin_id,a.customer_id,a.project_id,a.plugin_generation,a.grant_revision))
$$;
CREATE OR REPLACE FUNCTION webdock_rls.plugin_provisioning_status(provisioning_id text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
 SELECT jsonb_build_object('state',s.state,'resources',jsonb_strip_nulls(jsonb_build_object('appID',s.resources->>'appID')),'steps',coalesce((SELECT jsonb_agg(jsonb_build_object('name',step->>'name','state',step->>'state')) FROM jsonb_array_elements(s.steps) step),'[]'::jsonb))
 FROM webdock_auth.hosting_plugin_provisioning p JOIN webdock_auth.hosting_setup s ON s.id=p.setup_id AND s.project_id=p.project_id AND s.customer_id=p.customer_id
 WHERE p.id=$1 AND (webdock_rls.plugin_allows(p.plugin_id,p.customer_id,p.project_id,'plugin.apps.read') OR webdock_rls.plugin_allows(p.plugin_id,p.customer_id,p.project_id,'plugin.http.read') OR webdock_rls.plugin_allows(p.plugin_id,p.customer_id,p.project_id,'plugin.http.request'))
$$;
ALTER FUNCTION webdock_rls.plugin_provisioning_status(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.plugin_provisioning_status(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.plugin_provisioning_status(text) TO webdock_auth_runtime,webdock_worker_runtime;
CREATE OR REPLACE FUNCTION webdock_rls.plugin_provisioning_actor(provisioning_id text) RETURNS jsonb
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE result jsonb;
BEGIN
 IF NOT webdock_rls.plugin_provisioning_live(provisioning_id) THEN RETURN NULL; END IF;
 SELECT jsonb_build_object('actor',p.actor,'customerID',p.customer_id,
  'identity',jsonb_build_object('id',u.id,'name',u.name,'email',u.email,'role',u.role,'banned',u.banned,'emailVerified',u."emailVerified",'enabled',i.enabled),
  'organization',CASE WHEN co.organization_id IS NOT NULL THEN jsonb_build_object('id',co.organization_id,'customerID',p.customer_id,'active',true,'enabled',co.enabled) ELSE NULL END,
  'legacyAllowed',p.actor->'consoleIdentity' IS NULL AND session.id IS NOT NULL AND session."expiresAt">statement_timestamp() AND NOT coalesce(u.banned,false) AND u."emailVerified" AND NOT u."mustChangePassword"
    AND ((u.role='operator' AND u."twoFactorEnabled") OR (u.role='user' AND m.role IN ('owner','admin') AND h.mode='selfservice'))
    AND NOT EXISTS(SELECT 1 FROM webdock_auth.studio_tenant_preview preview WHERE preview.session_id=session.id)) INTO result
 FROM webdock_auth.hosting_plugin_provisioning p
 JOIN webdock_auth."user" u ON u.id=p.actor->>'subject'
 JOIN webdock_auth.tenant_customer tenant ON tenant.customer_id=p.customer_id
 JOIN webdock_auth.hosting_project h ON h.project_id=p.project_id
 LEFT JOIN webdock_auth.session session ON session.id=p.actor->>'sessionID' AND session."userId"=u.id
 LEFT JOIN webdock_auth.member m ON m."userId"=u.id AND m."organizationId"=tenant.organization_id
 LEFT JOIN webdock_auth.clerk_identity i ON i.user_id=u.id AND i.issuer=p.actor->'consoleIdentity'->>'issuer' AND i.clerk_user_id=p.actor->'consoleIdentity'->>'userID'
 LEFT JOIN webdock_auth.clerk_organization co ON co.issuer=i.issuer AND co.clerk_organization_id=p.actor->'consoleIdentity'->>'organizationID' AND co.organization_id=tenant.organization_id
 WHERE p.id=provisioning_id AND webdock_rls.agent_project(p.project_id)
 AND NOT EXISTS(SELECT 1 FROM webdock_auth.console_tenant_preview preview WHERE preview.session_id=p.actor->>'sessionID' AND preview.actor_id=p.actor->>'subject' AND preview.issuer=p.actor->'consoleIdentity'->>'issuer');
 RETURN result;
END $$;
ALTER FUNCTION webdock_rls.plugin_provisioning_actor(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.plugin_provisioning_actor(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.plugin_provisioning_actor(text) TO webdock_worker_runtime;
CREATE OR REPLACE FUNCTION webdock_rls.plugin_setup_operation_binding(operation_id text) RETURNS text
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
 SELECT p.id FROM webdock_auth.hosting_plugin_provisioning p JOIN webdock_auth.hosting_operation o ON o.project_id=p.project_id
 WHERE o.id=$1 AND (o.idempotency_key LIKE 'setup-'||p.setup_id||'-%'
 OR EXISTS(SELECT 1 FROM webdock_auth.hosting_operation receipt WHERE receipt.project_id=p.project_id AND receipt.idempotency_key LIKE 'setup-'||p.setup_id||'-%' AND receipt.result->>'operationID'=o.id)) LIMIT 1
$$;
CREATE OR REPLACE FUNCTION webdock_rls.plugin_setup_operation_live(operation_id text) RETURNS boolean
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
 SELECT CASE WHEN webdock_rls.plugin_setup_operation_binding($1) IS NULL THEN true ELSE webdock_rls.plugin_provisioning_live(webdock_rls.plugin_setup_operation_binding($1)) END
$$;
ALTER FUNCTION webdock_rls.plugin_setup_operation_binding(text) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.plugin_setup_operation_live(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.plugin_setup_operation_binding(text),webdock_rls.plugin_setup_operation_live(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.plugin_setup_operation_binding(text),webdock_rls.plugin_setup_operation_live(text) TO webdock_worker_runtime;
ALTER FUNCTION webdock_rls.plugin_provisioning_scope(text,text,text,integer,integer) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.plugin_provisioning_live(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.plugin_provisioning_scope(text,text,text,integer,integer),webdock_rls.plugin_provisioning_live(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.plugin_provisioning_scope(text,text,text,integer,integer),webdock_rls.plugin_provisioning_live(text) TO webdock_auth_runtime,webdock_worker_runtime;
`;
export function pluginProvisioningPolicy(table: string) {
  const scope = (cap: string) =>
    `webdock_rls.plugin_allows("${table}".plugin_id,"${table}".customer_id,"${table}".project_id,'${cap}')`;
  const write = scope("plugin.apps.provision");
  const read = `(${write} OR ${scope("plugin.apps.read")} OR ${scope("plugin.http.read")} OR ${scope("plugin.http.request")})`;
  const worker =
    table === "hosting_plugin_provisioning"
      ? `webdock_rls.plugin_provisioning_live("${table}".id)`
      : "false";
  // Setup needs to see revoked records to block instead of treating them as ordinary setups.
  const workerRead =
    table === "hosting_plugin_provisioning"
      ? `((webdock_rls.worker(ARRAY['hosting-setup']) AND "${table}".setup_id=webdock_rls.request_scope()->>'setupID') OR webdock_rls.agent_project("${table}".project_id))`
      : "false";
  return { read, write, workerRead, workerWrite: worker };
}
