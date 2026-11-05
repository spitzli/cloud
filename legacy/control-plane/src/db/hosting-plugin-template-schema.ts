// Installed after provisioning RLS helpers by the offline schema owner.
export const pluginTemplateSchemaSQL = `
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
`;
