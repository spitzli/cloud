GRANT USAGE,CREATE ON SCHEMA webdock_rls TO webdock_platform_owner;
CREATE FUNCTION webdock_rls.setup_build_capabilities(project text) RETURNS jsonb LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
BEGIN
 IF NOT webdock_rls.project_allowed(project) THEN RAISE EXCEPTION 'Project access required' USING ERRCODE='42501'; END IF;
 RETURN jsonb_build_object('isolated',EXISTS(SELECT 1 FROM webdock_auth.git_worker WHERE enabled AND isolation='microvm'),'githubActions',EXISTS(SELECT 1 FROM webdock_auth.git_worker WHERE enabled AND isolation='artifact-only'));
END $$;
ALTER FUNCTION webdock_rls.setup_build_capabilities(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.setup_build_capabilities(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.setup_build_capabilities(text) TO webdock_auth_runtime,webdock_worker_runtime;
REVOKE CREATE ON SCHEMA webdock_rls FROM webdock_platform_owner;
