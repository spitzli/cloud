/** Offline-installed, signed-scope check. SECURITY DEFINER prevents recursive RLS on its narrow metadata reads. */
export const pluginScopeSQL = `
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
`;

// Retained installer must preserve the same native transaction fence during overlap.
export const pluginInvocationFenceSQL = `
-- Native0017. Fixed signed invocation context; no caller-selected tenant or purpose.
CREATE OR REPLACE FUNCTION webdock_rls.lock_plugin_invocation() RETURNS boolean
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE before_scope jsonb; current_scope jsonb; v webdock_auth.plugin_invocation%ROWTYPE;
 p webdock_auth.plugin_installation%ROWTYPE; r webdock_auth.plugin_runtime%ROWTYPE;
 config webdock_auth.plugin_configuration%ROWTYPE; release webdock_auth.plugin_release%ROWTYPE;
 snapshot jsonb; action jsonb;
BEGIN
 before_scope:=webdock_rls.plugin_context(NULL);
 IF before_scope IS NULL THEN RETURN false; END IF;
 SELECT * INTO v FROM webdock_auth.plugin_invocation WHERE id::text=webdock_rls.request_scope()->>'pluginInvocationID';
 IF NOT FOUND THEN RETURN false; END IF;
 -- Shared metadata lock order: customer, project, installation, configuration, runtime,
 -- release, grant, optional job, invocation. Controllers must use the same order.
 PERFORM id FROM webdock_admin.customers WHERE id=v.customer_id FOR SHARE;
 PERFORM id FROM webdock_admin.projects WHERE id=v.project_id AND customer_id=v.customer_id FOR SHARE;
 SELECT * INTO p FROM webdock_auth.plugin_installation WHERE id=v.plugin_id FOR SHARE;
 SELECT * INTO config FROM webdock_auth.plugin_configuration WHERE plugin_id=v.plugin_id FOR SHARE;
 SELECT * INTO r FROM webdock_auth.plugin_runtime WHERE plugin_id=v.plugin_id AND generation=v.runtime_generation FOR SHARE;
 SELECT * INTO release FROM webdock_auth.plugin_release WHERE plugin_id=v.plugin_id AND version=p.version FOR SHARE;
 PERFORM plugin_id FROM webdock_auth.plugin_grant WHERE plugin_id=v.plugin_id AND customer_id=v.customer_id AND project_id=v.project_id FOR SHARE;
 IF v.kind='job' THEN
  PERFORM id FROM webdock_auth.plugin_job_execution WHERE id=v.job_execution_id FOR SHARE;
 END IF;
 SELECT * INTO v FROM webdock_auth.plugin_invocation WHERE id=v.id FOR SHARE;
 current_scope:=webdock_rls.plugin_context(NULL);
 IF current_scope IS NULL OR current_scope IS DISTINCT FROM before_scope THEN RETURN false; END IF;
 IF release.manifest_sha256 IS DISTINCT FROM encode(sha256(convert_to(release.manifest_bytes,'UTF8')),'hex')
  OR release.image IS DISTINCT FROM r.image OR release.version IS DISTINCT FROM r.version THEN RETURN false; END IF;
 IF v.kind='action' THEN
  SELECT value INTO action FROM jsonb_array_elements(release.manifest_bytes::jsonb->'actions') WHERE value->>'name'=v.name;
  IF action IS NULL OR action->>'intent' IS DISTINCT FROM v.intent OR action->'capabilities' IS DISTINCT FROM v.capabilities THEN RETURN false; END IF;
 END IF;
 snapshot:=v.session_context->'nativeAdmission';
 IF snapshot IS NOT NULL AND (
  snapshot->'v' IS DISTINCT FROM '1'::jsonb
  OR snapshot->'configurationRevision' IS DISTINCT FROM to_jsonb(config.revision)
  OR snapshot->'configuration' IS DISTINCT FROM config.configuration
  OR snapshot->>'manifestSHA256' IS DISTINCT FROM release.manifest_sha256
  OR snapshot->>'runtimeCredentialSHA256' IS DISTINCT FROM r.credential_hash
  OR snapshot->>'runtimeNamespace' IS DISTINCT FROM r.namespace
  OR snapshot->>'runtimeService' IS DISTINCT FROM r.service_name
 ) THEN RETURN false; END IF;
 RETURN true;
END $$;
ALTER FUNCTION webdock_rls.lock_plugin_invocation() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.lock_plugin_invocation() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.lock_plugin_invocation() TO webdock_auth_runtime,webdock_worker_runtime;
`;

export const mailPluginPolicySQL = `
-- Native0019. Tenant-only Mail grant; the caller must also prove its durable Mail job.
CREATE OR REPLACE FUNCTION webdock_rls.mail_plugin_policy(customer text, continuation boolean) RETURNS jsonb
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE binding webdock_mail.provider_binding%ROWTYPE; transport webdock_mail.send_transport%ROWTYPE;
 installation webdock_auth.plugin_installation%ROWTYPE; runtime webdock_auth.plugin_runtime%ROWTYPE;
 config webdock_auth.plugin_configuration%ROWTYPE; release webdock_auth.plugin_release%ROWTYPE;
 module_state text; selected_plugin text; manifest jsonb; action jsonb;
BEGIN
 IF NOT webdock_rls.worker(ARRAY['mail-send-gateway']) OR continuation IS NULL THEN RETURN NULL; END IF;
 PERFORM pg_advisory_xact_lock_shared(hashtext('webdock-plugin-installation'));
 -- Match invocation metadata order. Mail callers lock customer/transport before their job.
 PERFORM id FROM webdock_admin.customers WHERE id=customer AND status='active' FOR SHARE;
 IF NOT FOUND THEN RETURN NULL; END IF;
 SELECT * INTO transport FROM webdock_mail.send_transport WHERE customer_id=customer FOR SHARE;
 IF NOT FOUND OR transport.state<>'ready' THEN RETURN NULL; END IF;
 SELECT state INTO module_state FROM webdock_auth.plugin_installation WHERE id='mail' AND kind='builtin' FOR SHARE;
 IF module_state IS NULL OR (module_state<>'enabled' AND NOT(continuation AND module_state='draining')) THEN RETURN NULL; END IF;
 SELECT plugin_id INTO selected_plugin FROM webdock_mail.provider_binding WHERE customer_id=customer;
 IF selected_plugin IS NULL THEN RETURN NULL; END IF;
 SELECT * INTO installation FROM webdock_auth.plugin_installation WHERE id=selected_plugin AND kind='external' FOR SHARE;
 IF NOT FOUND OR installation.runtime<>'ready' OR (installation.state<>'enabled' AND NOT(continuation AND installation.state='draining')) THEN RETURN NULL; END IF;
 SELECT * INTO config FROM webdock_auth.plugin_configuration WHERE plugin_id=selected_plugin FOR SHARE;
 IF NOT FOUND THEN RETURN NULL; END IF;
 SELECT * INTO runtime FROM webdock_auth.plugin_runtime WHERE plugin_id=selected_plugin AND generation=installation.current_generation FOR SHARE;
 IF NOT FOUND OR runtime.configuration_revision IS DISTINCT FROM config.revision OR (runtime.status<>'ready' AND NOT(continuation AND runtime.status='draining')) THEN RETURN NULL; END IF;
 SELECT * INTO release FROM webdock_auth.plugin_release WHERE plugin_id=selected_plugin AND version=installation.version FOR SHARE;
 IF NOT FOUND OR release.image IS DISTINCT FROM runtime.image OR release.version IS DISTINCT FROM runtime.version
  OR release.manifest_sha256 IS DISTINCT FROM encode(sha256(convert_to(release.manifest_bytes,'UTF8')),'hex') THEN RETURN NULL; END IF;
 SELECT * INTO binding FROM webdock_mail.provider_binding WHERE customer_id=customer FOR SHARE;
 IF NOT FOUND OR binding.plugin_id IS DISTINCT FROM selected_plugin OR binding.revoked_at IS NOT NULL
  OR binding.transport_revision IS DISTINCT FROM transport.revision THEN RETURN NULL; END IF;
 manifest:=release.manifest_bytes::jsonb;
 IF manifest->>'name' IS DISTINCT FROM selected_plugin OR manifest->>'version' IS DISTINCT FROM installation.version
  OR manifest->>'image' IS DISTINCT FROM release.image THEN RETURN NULL; END IF;
 SELECT value INTO action FROM jsonb_array_elements(manifest->'actions') WHERE value->>'name'='mail.send';
 IF action IS NULL OR action->>'intent' IS DISTINCT FROM 'write' OR action ? 'httpBinding'
  OR jsonb_typeof(action->'capabilities') IS DISTINCT FROM 'array'
  OR NOT(config.approved_capabilities @> (action->'capabilities'))
  OR EXISTS(SELECT 1 FROM jsonb_array_elements_text(action->'capabilities') c WHERE c NOT IN ('plugin.http.read','plugin.http.request'))
  OR EXISTS(SELECT 1 FROM jsonb_array_elements(manifest->'services'->'required') s WHERE s->>'name'<>'http' OR s->>'version'<>'1')
 THEN RETURN NULL; END IF;
 RETURN jsonb_build_object('binding',to_jsonb(binding),'transport',jsonb_build_object('customer_id',transport.customer_id,'revision',transport.revision,'state',transport.state,'mode',transport.mode,'region',transport.region,'byok_revision',transport.byok_revision),
  'installation',to_jsonb(installation),'runtime',to_jsonb(runtime),'configuration',to_jsonb(config),'release',to_jsonb(release));
END $$;
ALTER FUNCTION webdock_rls.mail_plugin_policy(text,boolean) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.mail_plugin_policy(text,boolean) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.mail_plugin_policy(text,boolean) TO webdock_worker_runtime;
`;
