GRANT USAGE,CREATE ON SCHEMA webdock_rls TO webdock_platform_owner;
CREATE FUNCTION webdock_rls.project_without_setup(project text) RETURNS void LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
BEGIN
 IF NOT webdock_rls.operator() THEN RAISE EXCEPTION 'Operator required' USING ERRCODE='42501'; END IF;
 IF EXISTS(SELECT 1 FROM webdock_auth.hosting_setup WHERE project_id=project) THEN RAISE EXCEPTION 'Application setup history prevents deletion'; END IF;
END $$;
CREATE FUNCTION webdock_rls.setup_project_available(project text) RETURNS boolean LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
BEGIN
 RETURN webdock_rls.project_allowed(project,true) AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion WHERE project_id=project AND status<>'deleted');
END $$;
ALTER FUNCTION webdock_rls.project_without_setup(text) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.setup_project_available(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.project_without_setup(text),webdock_rls.setup_project_available(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.project_without_setup(text),webdock_rls.setup_project_available(text) TO webdock_auth_runtime,webdock_registry_runtime,webdock_worker_runtime;
REVOKE CREATE ON SCHEMA webdock_rls FROM webdock_platform_owner;
