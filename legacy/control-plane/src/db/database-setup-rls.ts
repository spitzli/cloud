export const setupHelperSQL = String.raw`
CREATE OR REPLACE FUNCTION webdock_rls.setup_allowed(customer text,project text DEFAULT NULL) RETURNS boolean LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
BEGIN
 IF NOT webdock_rls.worker(ARRAY['hosting-setup']) THEN RETURN false; END IF;
 RETURN EXISTS(
 SELECT 1 FROM webdock_auth.hosting_setup s
 JOIN webdock_admin.projects p ON p.id=s.project_id
 JOIN webdock_admin.customers c ON c.id=s.customer_id
 JOIN webdock_auth.hosting_project h ON h.project_id=p.id
 JOIN webdock_auth.tenant_customer t ON t.customer_id=c.id
 JOIN webdock_auth."user" u ON u.id=s.actor->>'subject'
 JOIN webdock_auth.session session ON session.id=s.actor->>'sessionID' AND session."userId"=u.id
 LEFT JOIN webdock_auth.member m ON m."userId"=u.id AND m."organizationId"=t.organization_id
 WHERE s.id=webdock_rls.request_scope()->>'setupID' AND s.generation::text=webdock_rls.request_scope()->>'generation'
 AND s.customer_id=customer AND (project IS NULL OR s.project_id=project) AND s.customer_id=webdock_rls.request_scope()->>'customerID'
 AND s.state='running' AND s.confirmed_preview_id IS NOT NULL AND s.lease_until>now()
 AND s.actor->'scopes' ? 'hosting:write' AND s.actor->'scopes' ? 'hosting:read'
 AND c.status='active' AND p.status='active' AND session."expiresAt">now()
 AND NOT coalesce(u.banned,false) AND u."emailVerified" AND NOT u."mustChangePassword"
 AND ((u.role='operator' AND u."twoFactorEnabled") OR (u.role='user' AND m.role IN ('owner','admin') AND h.mode='selfservice'))
 AND NOT EXISTS(SELECT 1 FROM webdock_auth.studio_tenant_preview preview WHERE preview.session_id=session.id)
 AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion deletion WHERE deletion.project_id=s.project_id AND deletion.status<>'deleted'));
END $$;
CREATE OR REPLACE FUNCTION webdock_rls.setup_permission(customer text,project text,permission text) RETURNS boolean LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE proof jsonb;
BEGIN
 IF NOT webdock_rls.setup_allowed(customer,project) THEN RETURN false; END IF;
 proof:=webdock_rls.request_scope()->'clerk';
 IF proof IS NULL THEN RETURN true; END IF;
 IF proof->'operator'='true'::jsonb THEN RETURN true; END IF;
 IF jsonb_typeof(proof->'permissions') IS DISTINCT FROM 'array' THEN RETURN false; END IF;
 RETURN coalesce(proof->'permissions' ? permission,false);
END $$;
ALTER FUNCTION webdock_rls.setup_permission(text,text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.setup_permission(text,text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.setup_permission(text,text,text) TO webdock_worker_runtime,webdock_auth_runtime,webdock_policy_owner;
CREATE OR REPLACE FUNCTION webdock_rls.setup_identity(subject text,session text DEFAULT NULL) RETURNS boolean LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
BEGIN
 IF NOT webdock_rls.worker(ARRAY['hosting-setup']) THEN RETURN false; END IF;
 RETURN EXISTS(SELECT 1 FROM webdock_auth.hosting_setup s WHERE s.id=webdock_rls.request_scope()->>'setupID' AND s.actor->>'subject'=setup_identity.subject AND (setup_identity.session IS NULL OR s.actor->>'sessionID'=setup_identity.session) AND webdock_rls.setup_allowed(s.customer_id::text,s.project_id::text));
END $$;
CREATE OR REPLACE FUNCTION webdock_rls.setup_cluster(cluster text) RETURNS boolean LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
BEGIN
 IF NOT webdock_rls.worker(ARRAY['hosting-setup']) THEN RETURN false; END IF;
 RETURN EXISTS(SELECT 1 FROM webdock_auth.hosting_setup s JOIN webdock_auth.hosting_project h ON h.project_id=s.project_id WHERE s.id=webdock_rls.request_scope()->>'setupID' AND h.cluster_id=cluster AND webdock_rls.setup_allowed(s.customer_id::text,s.project_id::text));
END $$;
ALTER FUNCTION webdock_rls.setup_allowed(text,text) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.setup_identity(text,text) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.setup_cluster(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.setup_allowed(text,text),webdock_rls.setup_identity(text,text),webdock_rls.setup_cluster(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.setup_allowed(text,text),webdock_rls.setup_identity(text,text),webdock_rls.setup_cluster(text) TO webdock_worker_runtime,webdock_auth_runtime,webdock_policy_owner;
`;
export function setupPolicyFor(t: {
  schema: string;
  name: string;
  columns: readonly string[];
}): { read: string; write: string; update?: string } | null {
  const col = (name: string) => `"${t.name}"."${name}"::text`;
  const allowed = (customer = "customer_id", project?: string) =>
    `webdock_rls.setup_allowed(${col(customer)}${project ? "," + col(project) : ""})`;
  const permission = t.name.startsWith("hosting_database") ? "org:databases:manage" : ["git_build", "git_artifact", "git_release", "git_check_outbox"].includes(t.name) ? "org:deployments:create" : "org:projects:manage";
  const writable = () => `webdock_rls.setup_permission(${col("customer_id")},${col("project_id")},'${permission}')`;
  const read = (value: string) => ({ read: value, write: "false" });
  if (t.schema === "webdock_admin") {
    if (t.name === "customers") return read(allowed("id"));
    if (t.name === "projects") return read(allowed("customer_id", "id"));
    return null;
  }
  if (t.schema !== "webdock_auth") return null;
  if (t.name === "hosting_setup")
    return {
      read: `webdock_rls.worker(ARRAY['hosting-setup-dispatch']) OR ${allowed("customer_id", "project_id")}`,
      write: `webdock_rls.worker(ARRAY['hosting-setup-dispatch'])`,
    };
  if (t.name === "clerk_identity_mapping")
    return read(`webdock_rls.setup_identity(${col("local_subject")})`);
  if (t.name === "clerk_session_mapping")
    return read(
      `EXISTS(SELECT 1 FROM webdock_auth.clerk_identity_mapping m WHERE m.issuer=${col("issuer")} AND m.external_subject=${col("external_subject")} AND m.revoked_at IS NULL AND webdock_rls.setup_identity(m.local_subject,${col("local_session_id")}))`,
    );
  if (t.name === "user")
    return read(`webdock_rls.setup_identity(${col("id")})`);
  if (t.name === "session")
    return read(`webdock_rls.setup_identity(${col("userId")},${col("id")})`);
  if (t.name === "member")
    return read(`webdock_rls.setup_identity(${col("userId")})`);
  if (t.name === "studio_tenant_preview")
    return read(
      `webdock_rls.setup_identity(${col("actor_id")},${col("session_id")})`,
    );
  if (
    [
      "hosting_project",
      "hosting_database",
      "hosting_database_offer",
      "hosting_database_order",
    ].includes(t.name)
  )
    return read(allowed("customer_id", "project_id"));
  if (
    [
      "hosting_service_binding",
      "hosting_operation",
      "hosting_reservation",
      "hosting_database",
      "hosting_database_offer",
      "hosting_database_order",
      "git_source",
      "git_build",
      "git_artifact",
      "git_release",
    ].includes(t.name)
  )
    return {
      read: allowed("customer_id", "project_id"),
      write: writable(),
    };
  if (
    [
      "tenant_customer",
      "tenant_subscription",
      "hosting_limit",
      "hosting_byok_policy",
      "git_connection",
      "git_repository",
      "mail_connection",
      "mail_connection_domain",
      "mail_sender_domain",
    ].includes(t.name)
  )
    return read(allowed());
  if (
    ["hosting_cluster", "hosting_agent", "hosting_database_backend"].includes(
      t.name,
    )
  )
    return read(
      `webdock_rls.setup_cluster(${col(t.name === "hosting_cluster" ? "id" : "cluster_id")})`,
    );
  if (t.name === "hosting_app") {
    const value = `EXISTS(SELECT 1 FROM webdock_auth.hosting_project p WHERE p.project_id::text=${col("project_id")} AND webdock_rls.setup_allowed(p.customer_id::text,p.project_id::text))`;
    return { read: value, write: value.replace("setup_allowed(p.customer_id::text,p.project_id::text)", "setup_permission(p.customer_id::text,p.project_id::text,'org:projects:manage')").replace("setup_allowed(b.customer_id::text,b.project_id::text)", "setup_permission(b.customer_id::text,b.project_id::text,'org:deployments:create')") };
  }
  if (["platform_settings", "hosting_database_plan"].includes(t.name))
    return read(
      `webdock_rls.setup_allowed(webdock_rls.request_scope()->>'customerID')`,
    );
  if (t.name === "git_check_outbox") {
    const value = `EXISTS(SELECT 1 FROM webdock_auth.git_build b WHERE b.id::text=${col("build_id")} AND webdock_rls.setup_allowed(b.customer_id::text,b.project_id::text))`;
    return { read: value, write: value.replace("setup_allowed(p.customer_id::text,p.project_id::text)", "setup_permission(p.customer_id::text,p.project_id::text,'org:projects:manage')").replace("setup_allowed(b.customer_id::text,b.project_id::text)", "setup_permission(b.customer_id::text,b.project_id::text,'org:deployments:create')") };
  }
  if (t.name === "mail_api_key")
    return {
      read: allowed(),
      write: `permission='sending_access' AND EXISTS(SELECT 1 FROM webdock_auth.hosting_setup s WHERE s.id=webdock_rls.request_scope()->>'setupID' AND s.configuration->'mail'->>'connectionID'=${col("connection_id")} AND s.configuration->'mail'->>'domainID'=${col("domain_id")} AND webdock_rls.setup_allowed(s.customer_id::text,s.project_id::text))`,
      update: `EXISTS(SELECT 1 FROM webdock_auth.hosting_service_binding b WHERE (b.mail_key_id=${col("id")} OR b.retiring_mail_key_id=${col("id")}) AND webdock_rls.setup_allowed(b.customer_id::text,b.project_id::text))`,
    };
  if (["hosting_audit", "access_event"].includes(t.name))
    return {
      read: "false",
      write: `webdock_rls.setup_identity(${col(t.name === "hosting_audit" ? "subject" : "actor_id")})`,
    };
  return null;
}
