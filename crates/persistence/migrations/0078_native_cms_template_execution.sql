-- Native writers disable old ID defaults. Keep the internal OAuth row primary key distinct
-- from the retained public clientId, reserving it through Core before sealing the blueprint.
CREATE OR REPLACE FUNCTION webdock_auth.publish_plugin_template(provisioning_id text) RETURNS void
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path TO 'pg_catalog'
    AS $_$
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
 IF coalesce(s->>'nativeOauthClientRowID','') !~ '^[1-9][0-9]{0,18}$' OR (s->>'nativeOauthClientRowID')::numeric>9223372036854775807 OR s->>'nativeOauthClientRowID' IN (s->>'clientID',s->>'instanceID') THEN RAISE EXCEPTION 'Native allocated client row identity required'; END IF;
 INSERT INTO webdock_auth."oauthClient"(id,"clientId","clientSecret",name,"applicationType","redirectUris","postLogoutRedirectUris",scopes,"grantTypes","responseTypes","tokenEndpointAuthMethod","requirePKCE","skipConsent","enableEndSession",disabled,metadata,"createdAt","updatedAt")
 VALUES(s->>'nativeOauthClientRowID',s->>'clientID',s->>'oauthSecretHash',p.name,'web',jsonb_build_array((s->>'origin')||'/api/sso/callback'),jsonb_build_array((s->>'origin')||'/cms'),'["openid","profile","email"]','["authorization_code"]','["code"]','client_secret_post',true,true,true,false,jsonb_build_object('webdock_binding',s->>'instanceID','webdock_provisioning',p.id),now(),now());
 INSERT INTO webdock_auth.app_binding(id,client_id,label,organization_id) VALUES(s->>'instanceID',s->>'clientID',p.name,tenant_id);
END; $_$;



-- Native agents execute only sealed managed-CMS phases. Retained legacy publication/block
-- functions intentionally remain unavailable to runtime roles directly.
CREATE FUNCTION webdock_auth.publish_native_cms_template(provisioning text, expected_spec text, expected_generation bigint, expected_token text)
RETURNS void LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE p webdock_auth.hosting_plugin_provisioning; phase jsonb; migration jsonb;
BEGIN
 IF webdock_rls.worker(ARRAY['agent']::text[]) IS NOT TRUE
    OR expected_spec IS NULL OR expected_spec !~ '^[a-f0-9]{64}$'
    OR expected_generation IS NULL OR expected_generation<1 OR expected_generation>9007199254740991
    OR coalesce(expected_token,'') !~ '^[A-Za-z0-9_-]{43}$' THEN
  RAISE EXCEPTION 'Native CMS agent scope required' USING ERRCODE='42501';
 END IF;
 SELECT * INTO p FROM webdock_auth.hosting_plugin_provisioning WHERE id=provisioning FOR UPDATE;
 phase:=p.template_state->'publication';
 migration:=p.template_state->'migration';
 IF NOT FOUND OR p.plugin_id<>'managed-cms' OR p.template_id<>'managed-cms'
    OR webdock_rls.agent_project(p.project_id) IS NOT TRUE
    OR webdock_rls.plugin_provisioning_live(p.id) IS NOT TRUE
    OR jsonb_typeof(p.template_state->'nativeCms'->'publication') IS DISTINCT FROM 'string'
    OR phase->>'nativeSpecSha256' IS DISTINCT FROM expected_spec
    OR phase->'nativeReceipt'->>'specSha256' IS DISTINCT FROM expected_spec
    OR coalesce(phase->'nativeReceipt'->>'leaseSha256','') !~ '^[a-f0-9]{64}$'
    OR coalesce(phase->'nativeReceipt'->>'completionSha256','') !~ '^[a-f0-9]{64}$'
    OR phase->>'status' IS DISTINCT FROM 'succeeded'
    OR jsonb_typeof(phase->'nativeLease'->'generation') IS DISTINCT FROM 'number'
    OR (phase->'nativeLease'->>'generation')::numeric IS DISTINCT FROM expected_generation::numeric
    OR (phase->>'generation')::numeric IS DISTINCT FROM expected_generation::numeric
    OR phase->'nativeLease'->>'token' IS DISTINCT FROM expected_token
    OR coalesce(phase->'nativeLease'->>'signature','') !~ '^[a-f0-9]{128}$'
    OR jsonb_typeof(phase->'nativeLease'->'expiresAt') IS DISTINCT FROM 'number'
    OR (phase->'nativeLease'->>'expiresAt')::numeric<=extract(epoch FROM clock_timestamp())
    OR (phase->'nativeLease'->>'expiresAt')::numeric>extract(epoch FROM clock_timestamp())+300
    OR (webdock_rls.plugin_provisioning_actor(p.id)->>'legacyAllowed') IS DISTINCT FROM 'true'
    OR coalesce(migration->'proof'->>'jobUID','') !~ '^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$'
    OR coalesce(migration->'proof'->>'pvcUID','') !~ '^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$'
    OR migration->'proof'->>'specSha256' IS DISTINCT FROM migration->'nativeReceipt'->>'specSha256'
    OR migration->'nativeCheckpoint'->>'migrationSucceeded' IS DISTINCT FROM 'true'
    OR jsonb_typeof(p.template_state->'nativeCms'->'migration') IS DISTINCT FROM 'string'
    OR coalesce(migration->>'nativeSpecSha256','') !~ '^[a-f0-9]{64}$'
    OR migration->>'nativeSpecSha256' IS DISTINCT FROM migration->'proof'->>'specSha256'
    OR migration->'nativeCheckpoint'->>'jobUID' IS DISTINCT FROM migration->'proof'->>'jobUID'
    OR coalesce(phase->'proof'->>'certificateUID','') !~ '^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$'
    OR coalesce(phase->'proof'->>'ingressUID','') !~ '^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$'
    OR phase->'proof'->>'certificateUID' IS DISTINCT FROM phase->'nativeCheckpoint'->>'certificateUID'
    OR phase->'proof'->>'ingressUID' IS DISTINCT FROM phase->'nativeCheckpoint'->>'ingressUID'
    OR NOT EXISTS(SELECT 1 FROM webdock_auth.hosting_project h JOIN webdock_auth.hosting_agent a ON a.cluster_id=h.cluster_id
      WHERE h.project_id=p.project_id AND h.customer_id=p.customer_id AND NOT a.revoked
       AND h.cluster_id=webdock_rls.request_scope()->>'clusterID'
       AND to_jsonb(a.generation) IS NOT DISTINCT FROM phase->'nativeLease'->'agentGeneration') THEN
  RAISE EXCEPTION 'Native CMS publication custody changed' USING ERRCODE='42501';
 END IF;
 PERFORM webdock_auth.publish_plugin_template(p.id);
END $$;
ALTER FUNCTION webdock_auth.publish_native_cms_template(text,text,bigint,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.publish_native_cms_template(text,text,bigint,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_auth.publish_native_cms_template(text,text,bigint,text) TO webdock_worker_runtime;

CREATE FUNCTION webdock_auth.block_native_cms_template(provisioning text, stage text, reason text)
RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE p webdock_auth.hosting_plugin_provisioning;
BEGIN
 IF webdock_rls.worker(ARRAY['agent']::text[]) IS NOT TRUE OR stage IS NULL OR stage NOT IN ('migration','publication')
    OR reason IS NULL OR reason NOT IN ('authority_changed','initialization_unavailable') THEN
  RAISE EXCEPTION 'Native CMS agent scope required' USING ERRCODE='42501';
 END IF;
 SELECT * INTO p FROM webdock_auth.hosting_plugin_provisioning WHERE id=provisioning FOR UPDATE;
 IF NOT FOUND OR p.plugin_id<>'managed-cms' OR p.template_id<>'managed-cms'
    OR webdock_rls.agent_project(p.project_id) IS NOT TRUE
    OR jsonb_typeof(p.template_state->'nativeCms'->stage) IS DISTINCT FROM 'string'
    OR coalesce(p.template_state->stage->>'nativeSpecSha256','') !~ '^[a-f0-9]{64}$' THEN
  RAISE EXCEPTION 'Native CMS retained phase changed' USING ERRCODE='42501';
 END IF;
 RETURN webdock_auth.block_template_operation(p.id,stage,reason);
END $$;
ALTER FUNCTION webdock_auth.block_native_cms_template(text,text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.block_native_cms_template(text,text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_auth.block_native_cms_template(text,text,text) TO webdock_worker_runtime;
