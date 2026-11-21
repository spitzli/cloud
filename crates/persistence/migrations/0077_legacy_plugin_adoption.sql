-- Explicit operator migration checks must not grant runtime SELECT on domain/workload tables.
CREATE FUNCTION webdock_auth.legacy_plugin_adoption_idle(plugin text) RETURNS boolean
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 IF NOT webdock_rls.operator() OR plugin IS NULL OR plugin !~ '^[a-z][a-z0-9-]{0,63}$' THEN RETURN false; END IF;
 PERFORM pg_advisory_xact_lock(hashtext('webdock-plugin-installation'));
 PERFORM id FROM webdock_auth.plugin_installation WHERE id=plugin FOR UPDATE;
 IF NOT FOUND THEN RETURN false; END IF;
 RETURN NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_install_execution WHERE plugin_id=plugin)
 AND NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_operation WHERE plugin_id=plugin AND status IN ('pending','running','blocked'))
 AND NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_invocation WHERE plugin_id=plugin AND revoked_at IS NULL AND expires_at>clock_timestamp())
 AND NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_job_execution WHERE plugin_id=plugin AND status IN ('pending','running'))
 AND NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_workload WHERE plugin_id=plugin)
 AND NOT EXISTS(SELECT 1 FROM webdock_mail.send_job j WHERE
  (plugin='mail' OR j.route->>'pluginID'=plugin OR (j.route->>'pluginID' IS NULL AND EXISTS(
   SELECT 1 FROM webdock_mail.provider_binding b WHERE b.customer_id=j.customer_id AND b.plugin_id=plugin)))
  AND (j.state IN ('staging','queued','leased','preparation_review','handoff','unknown') OR j.payload_deleted_at IS NULL))
 AND NOT EXISTS(SELECT 1 FROM webdock_mail.provider_lease WHERE (plugin='mail' OR plugin_id=plugin) AND revoked_at IS NULL AND expires_at>clock_timestamp())
 -- Changing the builtin Mail module to a provider must not invalidate another live provider.
 AND (plugin<>'mail' OR NOT EXISTS(SELECT 1 FROM webdock_mail.provider_binding WHERE revoked_at IS NULL))
 AND NOT EXISTS(SELECT 1 FROM webdock_auth.repository_lease WHERE plugin_id=plugin AND state='active' AND expires_at>clock_timestamp())
 AND NOT EXISTS(SELECT 1 FROM webdock_auth.git_build b JOIN webdock_auth.git_source s ON s.id=b.source_id
  JOIN webdock_auth.git_connection c ON c.id=s.connection_id WHERE c.repository_plugin_id=plugin AND b.status IN ('queued','running'))
 AND NOT EXISTS(SELECT 1 FROM webdock_auth.git_release r JOIN webdock_auth.git_source s ON s.id=r.source_id
  JOIN webdock_auth.git_connection c ON c.id=s.connection_id WHERE c.repository_plugin_id=plugin AND r.status IN ('deploying','needs-reconciliation'));
END $$;
ALTER FUNCTION webdock_auth.legacy_plugin_adoption_idle(text) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_auth.legacy_plugin_adoption_idle(text) SET SCHEMA webdock_rls;
REVOKE ALL ON FUNCTION webdock_rls.legacy_plugin_adoption_idle(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.legacy_plugin_adoption_idle(text) TO webdock_auth_runtime;
