-- Failure custody only. No runtime DDL, raw signer, allocation or data deletion is granted.
CREATE FUNCTION webdock_auth.block_native_cms_foundation(provisioning text, expected_spec text, reason text)
RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE p webdock_auth.hosting_plugin_provisioning;
BEGIN
 IF webdock_rls.worker(ARRAY['agent']::text[]) IS NOT TRUE
    OR coalesce(expected_spec,'') !~ '^[a-f0-9]{64}$'
    OR reason IS NULL OR reason NOT IN ('authority_changed','foundation_unavailable') THEN
  RAISE EXCEPTION 'Native CMS foundation agent scope required' USING ERRCODE='42501';
 END IF;
 SELECT * INTO p FROM webdock_auth.hosting_plugin_provisioning WHERE id=provisioning FOR UPDATE;
 IF NOT FOUND OR p.plugin_id<>'managed-cms' OR p.template_id<>'managed-cms'
    OR webdock_rls.agent_project(p.project_id) IS NOT TRUE
    OR jsonb_typeof(p.template_state->'nativeCms'->'foundation') IS DISTINCT FROM 'string'
    OR p.template_state->'foundation'->>'nativeSpecSha256' IS DISTINCT FROM expected_spec THEN
  RAISE EXCEPTION 'Native CMS foundation custody changed' USING ERRCODE='42501';
 END IF;
 IF coalesce(p.template_state->'foundation'->>'status','') NOT IN ('queued','running') THEN RETURN false; END IF;
 UPDATE webdock_auth.hosting_plugin_provisioning
 SET template_state=jsonb_set(template_state,'{foundation}',template_state->'foundation'||jsonb_build_object('status','needs-reconciliation','reason',reason)),updated_at=now()
 WHERE id=p.id;
 RETURN true;
END $$;
ALTER FUNCTION webdock_auth.block_native_cms_foundation(text,text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.block_native_cms_foundation(text,text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_auth.block_native_cms_foundation(text,text,text) TO webdock_worker_runtime;
