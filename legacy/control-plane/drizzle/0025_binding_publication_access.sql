CREATE POLICY binding_git_worker ON webdock_auth.hosting_service_binding TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['git-worker'])) WITH CHECK(webdock_rls.worker(ARRAY['git-worker']));
GRANT USAGE,CREATE ON SCHEMA webdock_rls TO webdock_platform_owner;
CREATE FUNCTION webdock_rls.binding_database_status(binding text) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
BEGIN
 RETURN (SELECT jsonb_build_object('state',d.state,'credential_revision',d.credential_revision,'confirmed_credential_revision',d.confirmed_credential_revision,'ca_certificate',b.ca_certificate,'endpoint_ready',b.endpoint_ready)
 FROM webdock_auth.hosting_service_binding s JOIN webdock_auth.hosting_database d ON d.id=s.service->>'databaseID' AND d.project_id=s.project_id AND d.customer_id=s.customer_id JOIN webdock_auth.hosting_database_backend b ON b.id=d.backend_id
 WHERE s.id=binding AND (webdock_rls.customer_allowed(s.customer_id::text) OR webdock_rls.worker(ARRAY['git-worker']) OR webdock_rls.agent_project(s.project_id::text)));
END $$;
ALTER FUNCTION webdock_rls.binding_database_status(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.binding_database_status(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.binding_database_status(text) TO webdock_auth_runtime,webdock_worker_runtime;
REVOKE CREATE ON SCHEMA webdock_rls FROM webdock_platform_owner;
