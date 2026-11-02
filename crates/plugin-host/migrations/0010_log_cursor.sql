-- Native0037: a stable cursor keeps every line when timestamps tie at a page boundary.
ALTER TABLE webdock_auth.workload_log ADD COLUMN id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY;
DROP INDEX webdock_auth.workload_log_resource;
CREATE INDEX workload_log_resource ON webdock_auth.workload_log(project_id, plugin_id, resource_id, at DESC, id DESC);
DROP FUNCTION webdock_rls.resource_logs(text,text,text,text,timestamptz);
CREATE FUNCTION webdock_rls.resource_logs(project text, plugin text, resource text, search text, before bigint) RETURNS jsonb
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE customer text;
BEGIN
 SELECT customer_id INTO customer FROM webdock_admin.projects WHERE id=project;
 IF customer IS NULL OR NOT webdock_rls.customer_allowed(customer,true) THEN RAISE EXCEPTION 'customer access required' USING ERRCODE='42501'; END IF;
 RETURN coalesce((SELECT jsonb_agg(jsonb_build_object('id',l.id::text,'at',l.at,'unit',l.unit,'line',l.line) ORDER BY l.at DESC,l.id DESC) FROM (
   SELECT * FROM webdock_auth.workload_log
   WHERE project_id=project AND plugin_id=plugin AND resource_id=resource
    AND at>=statement_timestamp()-interval '3 days'
    AND (before IS NULL OR (at,id)<(SELECT c.at,c.id FROM webdock_auth.workload_log c
         WHERE c.id=before AND c.project_id=project AND c.plugin_id=plugin AND c.resource_id=resource))
    AND (coalesce(search,'')='' OR strpos(lower(line),lower(search))>0)
   ORDER BY at DESC,id DESC LIMIT 500) l),'[]'::jsonb);
END $$;
ALTER FUNCTION webdock_rls.resource_logs(text,text,text,text,bigint) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.resource_logs(text,text,text,text,bigint) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.resource_logs(text,text,text,text,bigint) TO webdock_auth_runtime;
