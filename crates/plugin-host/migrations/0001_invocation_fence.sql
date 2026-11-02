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
