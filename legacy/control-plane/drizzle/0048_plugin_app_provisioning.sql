-- Durable approved application allocations; installed offline, retaining existing resources.
-- PostgreSQL requires the new function owner to have CREATE during ownership transfer.
GRANT USAGE,CREATE ON SCHEMA webdock_rls TO webdock_platform_owner;

CREATE TABLE IF NOT EXISTS webdock_auth.hosting_plugin_provisioning (
 id varchar PRIMARY KEY DEFAULT webdock_auth.next_snowflake(),
 plugin_id text NOT NULL,customer_id varchar NOT NULL,project_id varchar NOT NULL,
 template_id text NOT NULL,name text NOT NULL,actor jsonb NOT NULL,
 plugin_generation integer NOT NULL,grant_revision integer NOT NULL,
 setup_id varchar UNIQUE REFERENCES webdock_auth.hosting_setup(id),
 binding jsonb,template_state jsonb NOT NULL DEFAULT '{}',
 created_at timestamptz NOT NULL DEFAULT now(),updated_at timestamptz NOT NULL DEFAULT now(),
 UNIQUE(plugin_id,project_id,template_id),
 FOREIGN KEY(project_id,customer_id) REFERENCES webdock_admin.projects(id,customer_id)
);
CREATE TABLE IF NOT EXISTS webdock_auth.hosting_plugin_provisioning_request (
 plugin_id text NOT NULL,customer_id varchar NOT NULL,project_id varchar NOT NULL,
 request_id text NOT NULL,request_hash text NOT NULL,provisioning_id varchar NOT NULL REFERENCES webdock_auth.hosting_plugin_provisioning(id),
 PRIMARY KEY(plugin_id,project_id,request_id),
 FOREIGN KEY(project_id,customer_id) REFERENCES webdock_admin.projects(id,customer_id)
);


CREATE OR REPLACE FUNCTION webdock_rls.plugin_context(capability text) RETURNS jsonb
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $plugin$
DECLARE scope jsonb; invocation record; actor jsonb;
BEGIN
 scope:=webdock_rls.request_scope();
 IF scope IS NULL OR scope->>'pluginInvocationID' IS NULL THEN RETURN NULL; END IF;
 SELECT v.* INTO invocation FROM webdock_auth.plugin_invocation v
 JOIN webdock_auth.plugin_installation p ON p.id=v.plugin_id
 JOIN webdock_auth.plugin_runtime r ON r.plugin_id=p.id AND r.generation=p.current_generation
 JOIN webdock_auth.plugin_configuration config ON config.plugin_id=p.id
 JOIN webdock_auth.plugin_grant g ON g.plugin_id=v.plugin_id AND g.customer_id=v.customer_id AND g.project_id=v.project_id
 JOIN webdock_admin.customers c ON c.id=v.customer_id AND c.status='active'
 JOIN webdock_admin.projects project ON project.id=v.project_id AND project.customer_id=v.customer_id AND project.status='active'
 WHERE v.id::text=scope->>'pluginInvocationID' AND v.revoked_at IS NULL AND v.created_at<=statement_timestamp() AND v.expires_at>statement_timestamp() AND v.expires_at<=statement_timestamp()+interval '60 seconds'
 AND p.state IN ('enabled','draining') AND p.runtime='ready' AND r.status IN ('ready','draining')
 AND p.current_generation=v.runtime_generation AND r.configuration_revision=config.revision
 AND g.revoked_at IS NULL AND g.revision=v.grant_revision
 AND (capability IS NULL OR (config.approved_capabilities ? capability AND g.capabilities ? capability AND v.capabilities ? capability))
 AND (capability IS NULL OR capability IN ('plugin.apps.read','project.read','plugin.data.read','plugin.db.read','plugin.cache.read','plugin.storage.read','plugin.secrets.read','plugin.http.read') OR v.intent='write')
 AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=project.id AND d.status<>'deleted');
 IF NOT FOUND THEN RETURN NULL; END IF;
 IF invocation.kind='action' THEN
  actor:=webdock_rls.actor();
  IF actor IS NULL OR actor->>'subject' IS DISTINCT FROM invocation.actor_id OR actor->>'sessionID' IS DISTINCT FROM invocation.session_id
   OR actor->'preview' IS DISTINCT FROM 'null'::jsonb OR NOT webdock_rls.customer_allowed(invocation.customer_id,false) THEN RETURN NULL; END IF;
 ELSE
  IF NOT webdock_rls.worker(ARRAY['plugin-job']::text[]) OR NOT EXISTS(
   SELECT 1 FROM webdock_auth.plugin_job_execution e JOIN webdock_auth.plugin_grant g ON g.plugin_id=e.plugin_id AND g.customer_id=e.customer_id AND g.project_id=e.project_id
   WHERE e.id=invocation.job_execution_id AND e.plugin_id=invocation.plugin_id AND e.customer_id=invocation.customer_id AND e.project_id=invocation.project_id
   AND e.name=invocation.name AND g.jobs ? e.name AND e.grant_revision=invocation.grant_revision
   AND e.claim_token=invocation.job_claim_token AND e.runtime_generation=invocation.runtime_generation AND e.status='running' AND e.lease_until>statement_timestamp() AND NOT e.cancel_requested
  ) THEN RETURN NULL; END IF;
 END IF;
 RETURN jsonb_build_object('pluginID',invocation.plugin_id,'customerID',invocation.customer_id,'projectID',invocation.project_id,'operationID',invocation.operation_id);
END $plugin$;
ALTER FUNCTION webdock_rls.plugin_context(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.plugin_context(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.plugin_context(text) TO webdock_auth_runtime,webdock_worker_runtime;
CREATE OR REPLACE FUNCTION webdock_rls.plugin_allows(plugin text, customer text, project text, capability text) RETURNS boolean
LANGUAGE sql STABLE SECURITY INVOKER SET search_path=pg_catalog,webdock_rls AS $plugin$
 SELECT coalesce(webdock_rls.plugin_context(capability) @> jsonb_build_object('pluginID',plugin,'customerID',customer,'projectID',project),false)
$plugin$;
ALTER FUNCTION webdock_rls.plugin_allows(text,text,text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.plugin_allows(text,text,text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.plugin_allows(text,text,text,text) TO webdock_auth_runtime,webdock_worker_runtime;


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


CREATE OR REPLACE FUNCTION webdock_auth.publish_plugin_template(provisioning_id text)
RETURNS void LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE p record; s jsonb; tenant_id text; existing record;
BEGIN
 IF NOT webdock_rls.plugin_provisioning_live(provisioning_id) THEN RAISE EXCEPTION 'Provisioning authority changed' USING ERRCODE='42501'; END IF;
 SELECT * INTO STRICT p FROM webdock_auth.hosting_plugin_provisioning WHERE id=provisioning_id FOR UPDATE;
 s:=p.template_state;
 IF p.template_id<>'managed-cms' OR s->'migration'->>'status' IS DISTINCT FROM 'succeeded' OR s->'publication'->>'status' IS DISTINCT FROM 'succeeded'
 OR COALESCE(s->>'oauthSecretHash','') !~ '^[A-Za-z0-9_-]{43}$' OR COALESCE(s->>'instanceID','') !~ '^[1-9][0-9]{0,18}$' OR COALESCE(s->>'clientID','') !~ '^[1-9][0-9]{0,18}$'
 OR s->>'origin' IS DISTINCT FROM 'https://'||(s->>'instanceID')||'.'||(s->'approval'->>'domain')
 OR s->'publication'->'proof'->>'instanceID' IS DISTINCT FROM s->>'instanceID'
 OR s->'publication'->'proof'->>'origin' IS DISTINCT FROM s->>'origin'
 THEN RAISE EXCEPTION 'Verified instance publication required'; END IF;
 SELECT organization_id INTO STRICT tenant_id FROM webdock_auth.tenant_customer WHERE customer_id=p.customer_id;
 SELECT b.*,c."redirectUris",c.metadata INTO existing FROM webdock_auth.app_binding b JOIN webdock_auth."oauthClient" c ON c."clientId"=b.client_id WHERE b.id=s->>'instanceID';
 IF FOUND THEN
  IF existing.client_id IS DISTINCT FROM s->>'clientID' OR existing.organization_id IS DISTINCT FROM tenant_id
   OR existing."redirectUris" IS DISTINCT FROM jsonb_build_array((s->>'origin')||'/api/sso/callback')
   OR existing.metadata->>'webdock_provisioning' IS DISTINCT FROM p.id::text THEN RAISE EXCEPTION 'Website binding ownership changed'; END IF;
  RETURN;
 END IF;
 IF EXISTS(SELECT 1 FROM webdock_auth."oauthClient" WHERE "redirectUris" @> jsonb_build_array((s->>'origin')||'/api/sso/callback')) THEN RAISE EXCEPTION 'Origin already registered'; END IF;
 INSERT INTO webdock_auth."oauthClient"("clientId","clientSecret",name,"applicationType","redirectUris","postLogoutRedirectUris",scopes,"grantTypes","responseTypes","tokenEndpointAuthMethod","requirePKCE","skipConsent","enableEndSession",disabled,metadata,"createdAt","updatedAt")
 VALUES(s->>'clientID',s->>'oauthSecretHash',p.name,'web',jsonb_build_array((s->>'origin')||'/api/sso/callback'),jsonb_build_array((s->>'origin')||'/cms'),'["openid","profile","email"]','["authorization_code"]','["code"]','client_secret_post',true,true,true,false,jsonb_build_object('webdock_binding',s->>'instanceID','webdock_provisioning',p.id),now(),now());
 INSERT INTO webdock_auth.app_binding(id,client_id,label,organization_id) VALUES(s->>'instanceID',s->>'clientID',p.name,tenant_id);
END; $$;
ALTER FUNCTION webdock_auth.publish_plugin_template(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.publish_plugin_template(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_auth.publish_plugin_template(text) TO webdock_auth_runtime,webdock_worker_runtime;
DROP FUNCTION IF EXISTS webdock_auth.published_template_instances();
CREATE OR REPLACE FUNCTION webdock_auth.block_template_operation(provisioning_id text,phase text,reason_code text)
RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE p record; operation jsonb;
BEGIN
 IF phase NOT IN ('migration','publication') OR reason_code NOT IN ('authority_changed','initialization_unavailable') THEN RAISE EXCEPTION 'Invalid template block request'; END IF;
 SELECT * INTO p FROM webdock_auth.hosting_plugin_provisioning WHERE id=provisioning_id FOR UPDATE;
 IF NOT FOUND OR NOT webdock_rls.agent_project(p.project_id) THEN RAISE EXCEPTION 'Template agent scope changed' USING ERRCODE='42501'; END IF;
 operation:=p.template_state->phase;
 IF operation->>'status' NOT IN ('queued','running') OR operation IS NULL THEN RETURN false; END IF;
 IF operation->>'status'='running' AND (operation->>'leaseUntil')::timestamptz>now() THEN RETURN false; END IF;
 operation:=(operation-'leaseToken'-'leaseUntil')||jsonb_build_object('status','needs-reconciliation','reason',reason_code);
 UPDATE webdock_auth.hosting_plugin_provisioning SET template_state=jsonb_set(template_state,ARRAY[phase],operation),updated_at=now() WHERE id=p.id;
 RETURN true;
END; $$;
ALTER FUNCTION webdock_auth.block_template_operation(text,text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.block_template_operation(text,text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_auth.block_template_operation(text,text,text) TO webdock_worker_runtime;


ALTER TABLE webdock_auth.hosting_plugin_provisioning OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.hosting_plugin_provisioning ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.hosting_plugin_provisioning FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_auth.hosting_plugin_provisioning FROM PUBLIC;
GRANT SELECT,INSERT,UPDATE ON webdock_auth.hosting_plugin_provisioning TO webdock_auth_runtime,webdock_worker_runtime;
CREATE POLICY provision_owner ON webdock_auth.hosting_plugin_provisioning TO webdock_platform_owner USING(true) WITH CHECK(true);
CREATE POLICY provision_runtime_read ON webdock_auth.hosting_plugin_provisioning FOR SELECT TO webdock_auth_runtime USING((webdock_rls.plugin_allows("hosting_plugin_provisioning".plugin_id,"hosting_plugin_provisioning".customer_id,"hosting_plugin_provisioning".project_id,'plugin.apps.provision') OR webdock_rls.plugin_allows("hosting_plugin_provisioning".plugin_id,"hosting_plugin_provisioning".customer_id,"hosting_plugin_provisioning".project_id,'plugin.apps.read') OR webdock_rls.plugin_allows("hosting_plugin_provisioning".plugin_id,"hosting_plugin_provisioning".customer_id,"hosting_plugin_provisioning".project_id,'plugin.http.read') OR webdock_rls.plugin_allows("hosting_plugin_provisioning".plugin_id,"hosting_plugin_provisioning".customer_id,"hosting_plugin_provisioning".project_id,'plugin.http.request')));
CREATE POLICY provision_runtime_insert ON webdock_auth.hosting_plugin_provisioning FOR INSERT TO webdock_auth_runtime WITH CHECK(webdock_rls.plugin_allows("hosting_plugin_provisioning".plugin_id,"hosting_plugin_provisioning".customer_id,"hosting_plugin_provisioning".project_id,'plugin.apps.provision'));
CREATE POLICY provision_runtime_update ON webdock_auth.hosting_plugin_provisioning FOR UPDATE TO webdock_auth_runtime USING(webdock_rls.plugin_allows("hosting_plugin_provisioning".plugin_id,"hosting_plugin_provisioning".customer_id,"hosting_plugin_provisioning".project_id,'plugin.apps.provision')) WITH CHECK(webdock_rls.plugin_allows("hosting_plugin_provisioning".plugin_id,"hosting_plugin_provisioning".customer_id,"hosting_plugin_provisioning".project_id,'plugin.apps.provision'));
CREATE POLICY provision_worker_read ON webdock_auth.hosting_plugin_provisioning FOR SELECT TO webdock_worker_runtime USING(((webdock_rls.worker(ARRAY['hosting-setup']) AND "hosting_plugin_provisioning".setup_id=webdock_rls.request_scope()->>'setupID') OR webdock_rls.agent_project("hosting_plugin_provisioning".project_id)));
CREATE POLICY provision_worker_update ON webdock_auth.hosting_plugin_provisioning FOR UPDATE TO webdock_worker_runtime USING(webdock_rls.plugin_provisioning_live("hosting_plugin_provisioning".id)) WITH CHECK(webdock_rls.plugin_provisioning_live("hosting_plugin_provisioning".id));

ALTER TABLE webdock_auth.hosting_plugin_provisioning_request OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.hosting_plugin_provisioning_request ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.hosting_plugin_provisioning_request FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_auth.hosting_plugin_provisioning_request FROM PUBLIC;
GRANT SELECT,INSERT,UPDATE ON webdock_auth.hosting_plugin_provisioning_request TO webdock_auth_runtime,webdock_worker_runtime;
CREATE POLICY provision_owner ON webdock_auth.hosting_plugin_provisioning_request TO webdock_platform_owner USING(true) WITH CHECK(true);
CREATE POLICY provision_runtime_read ON webdock_auth.hosting_plugin_provisioning_request FOR SELECT TO webdock_auth_runtime USING((webdock_rls.plugin_allows("hosting_plugin_provisioning_request".plugin_id,"hosting_plugin_provisioning_request".customer_id,"hosting_plugin_provisioning_request".project_id,'plugin.apps.provision') OR webdock_rls.plugin_allows("hosting_plugin_provisioning_request".plugin_id,"hosting_plugin_provisioning_request".customer_id,"hosting_plugin_provisioning_request".project_id,'plugin.apps.read') OR webdock_rls.plugin_allows("hosting_plugin_provisioning_request".plugin_id,"hosting_plugin_provisioning_request".customer_id,"hosting_plugin_provisioning_request".project_id,'plugin.http.read') OR webdock_rls.plugin_allows("hosting_plugin_provisioning_request".plugin_id,"hosting_plugin_provisioning_request".customer_id,"hosting_plugin_provisioning_request".project_id,'plugin.http.request')));
CREATE POLICY provision_runtime_insert ON webdock_auth.hosting_plugin_provisioning_request FOR INSERT TO webdock_auth_runtime WITH CHECK(webdock_rls.plugin_allows("hosting_plugin_provisioning_request".plugin_id,"hosting_plugin_provisioning_request".customer_id,"hosting_plugin_provisioning_request".project_id,'plugin.apps.provision'));
CREATE POLICY provision_runtime_update ON webdock_auth.hosting_plugin_provisioning_request FOR UPDATE TO webdock_auth_runtime USING(false) WITH CHECK(false);
CREATE POLICY provision_worker_read ON webdock_auth.hosting_plugin_provisioning_request FOR SELECT TO webdock_worker_runtime USING(false);
CREATE POLICY provision_worker_update ON webdock_auth.hosting_plugin_provisioning_request FOR UPDATE TO webdock_worker_runtime USING(false) WITH CHECK(false);
CREATE POLICY provision_setup_agent_read ON webdock_auth.hosting_setup FOR SELECT TO webdock_worker_runtime USING(webdock_rls.agent_project(project_id) AND EXISTS(SELECT 1 FROM webdock_auth.hosting_plugin_provisioning provision WHERE provision.setup_id=hosting_setup.id));
REVOKE CREATE ON SCHEMA webdock_rls FROM webdock_platform_owner;
