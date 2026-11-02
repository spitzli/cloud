-- Native0036: an empty project's first workload and its cluster selection share the project lock.
CREATE OR REPLACE FUNCTION webdock_rls.project_cluster(project text) RETURNS text
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 IF NOT webdock_rls.worker(ARRAY['plugin-controller']::text[]) THEN RAISE EXCEPTION 'plugin controller required' USING ERRCODE='42501'; END IF;
 RETURN (SELECT cluster_id FROM webdock_admin.projects WHERE id=project FOR UPDATE);
END $$;

-- Expired logs are never readable, including quiet and already removed units.
CREATE OR REPLACE FUNCTION webdock_rls.resource_logs(project text, plugin text, resource text, search text, before timestamptz) RETURNS jsonb
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE customer text;
BEGIN
 SELECT customer_id INTO customer FROM webdock_admin.projects WHERE id=project;
 IF customer IS NULL OR NOT webdock_rls.customer_allowed(customer,true) THEN RAISE EXCEPTION 'customer access required' USING ERRCODE='42501'; END IF;
 RETURN coalesce((SELECT jsonb_agg(jsonb_build_object('at',l.at,'unit',l.unit,'line',l.line) ORDER BY l.at DESC) FROM (
   SELECT * FROM webdock_auth.workload_log
   WHERE project_id=project AND plugin_id=plugin AND resource_id=resource AND at<coalesce(before,'infinity')
    AND at>=statement_timestamp()-interval '3 days'
    AND (coalesce(search,'')='' OR strpos(lower(line),lower(search))>0)
   ORDER BY at DESC LIMIT 500) l),'[]'::jsonb);
END $$;

-- Periodic retention is independent of reports, units and outgoing-mail configuration.
CREATE FUNCTION webdock_rls.prune_plugin_history() RETURNS void
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 IF NOT webdock_rls.worker(ARRAY['plugin-controller']::text[]) THEN RAISE EXCEPTION 'plugin controller required' USING ERRCODE='42501'; END IF;
 IF NOT pg_try_advisory_xact_lock(hashtext('webdock-plugin-history')) THEN RETURN; END IF;
 DELETE FROM webdock_auth.workload_log WHERE at<statement_timestamp()-interval '3 days';
 DELETE FROM webdock_auth.workload_usage WHERE hour<statement_timestamp()-interval '35 days';
 DELETE FROM webdock_auth.notification WHERE resolved_at<statement_timestamp()-interval '7 days';
END $$;
ALTER FUNCTION webdock_rls.prune_plugin_history() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.prune_plugin_history() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.prune_plugin_history() TO webdock_worker_runtime;
