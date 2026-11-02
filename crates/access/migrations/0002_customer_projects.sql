-- Native0027: customer owners and admins create projects in their own workspace (self-service).
-- The runtime role keeps SELECT only on projects; this function is the single customer write path.
CREATE FUNCTION webdock_rls.create_customer_project(project text, project_name text, customer text) RETURNS boolean
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 IF webdock_rls.actor() IS NULL OR webdock_rls.actor()->'preview' IS DISTINCT FROM 'null'::jsonb
  OR NOT webdock_rls.customer_allowed(customer, true)
  OR NOT EXISTS(SELECT 1 FROM webdock_admin.customers WHERE id=customer AND status='active') THEN RETURN false; END IF;
 INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES(project,project_name,customer);
 RETURN true;
END $$;
ALTER FUNCTION webdock_rls.create_customer_project(text,text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.create_customer_project(text,text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.create_customer_project(text,text,text) TO webdock_auth_runtime;
