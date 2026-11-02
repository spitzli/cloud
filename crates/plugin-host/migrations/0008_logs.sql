-- Native0034: log history. Core keeps each unit's log lines for 3 days, appended from the newest lines
-- the agent sends with every report, so customers can look back past the last 100 and search them.
-- Logs often carry configuration values: only owners, admins and operators read them.
CREATE TABLE webdock_auth.workload_log (
 plugin_id text NOT NULL,
 unit text NOT NULL,
 project_id text NOT NULL,
 resource_id text,
 at timestamptz NOT NULL,
 line text NOT NULL CHECK (char_length(line)<=4096)
);
CREATE INDEX workload_log_unit ON webdock_auth.workload_log(plugin_id, unit, at);
CREATE INDEX workload_log_resource ON webdock_auth.workload_log(project_id, plugin_id, resource_id, at);
CREATE INDEX workload_log_age ON webdock_auth.workload_log(at);
ALTER TABLE webdock_auth.workload_log OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.workload_log ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.workload_log FORCE ROW LEVEL SECURITY;
REVOKE ALL ON TABLE webdock_auth.workload_log FROM PUBLIC;
CREATE POLICY platform_owner ON webdock_auth.workload_log TO webdock_platform_owner USING (true) WITH CHECK (true);

CREATE OR REPLACE FUNCTION webdock_rls.report_plugin_workload(plugin text, workload text, reported bigint, state jsonb, failure text, removed boolean) RETURNS boolean
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE w webdock_auth.plugin_workload; cpu bigint; memory bigint; customer text;
BEGIN
 SELECT * INTO w FROM webdock_auth.plugin_workload WHERE plugin_id=plugin AND unit=workload FOR UPDATE;
 IF NOT FOUND OR NOT webdock_rls.worker(ARRAY['agent']::text[], w.cluster_id::text) OR reported>w.revision OR reported<1 THEN RETURN false; END IF;
 IF removed THEN
  IF NOT w.removing OR reported<>w.revision THEN RETURN false; END IF;
  PERFORM webdock_auth.notice(w, ARRAY[]::text[]);
  DELETE FROM webdock_auth.plugin_workload WHERE plugin_id=plugin AND unit=workload;
  RETURN true;
 END IF;
 IF state IS NULL OR jsonb_typeof(state)<>'object' OR octet_length(state::text)>65536 THEN RETURN false; END IF;
 UPDATE webdock_auth.plugin_workload SET applied_revision=greatest(applied_revision,reported), observed=state,
  error=left(failure,500), observed_at=statement_timestamp() WHERE plugin_id=plugin AND unit=workload;
 -- Only reports of the current revision speak for the unit; a removal in progress needs no attention.
 IF reported=w.revision AND NOT w.removing THEN
  PERFORM webdock_auth.notice(w, webdock_auth.attention(state, failure));
 END IF;
 -- New log lines since the last stored one; the agent sends the newest lines with every report.
 INSERT INTO webdock_auth.workload_log(plugin_id,unit,project_id,resource_id,at,line)
  SELECT plugin,workload,w.project_id,w.resource_id,m[1]::timestamptz,left(m[2],4096)
  FROM jsonb_array_elements_text(CASE WHEN jsonb_typeof(state->'logs')='array' THEN state->'logs' ELSE '[]'::jsonb END) l,
   regexp_match(l,'^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?Z) ?(.*)$') m
  WHERE m IS NOT NULL AND m[1]::timestamptz>coalesce((SELECT max(at) FROM webdock_auth.workload_log WHERE plugin_id=plugin AND unit=workload),'-infinity');
 IF FOUND THEN
  DELETE FROM webdock_auth.workload_log WHERE at<statement_timestamp()-interval '3 days';
 END IF;
 IF state->'usage'->>'cpu' ~ '^[0-9]{1,12}$' AND state->'usage'->>'memory' ~ '^[0-9]{1,12}$' THEN
  cpu:=(state->'usage'->>'cpu')::bigint; memory:=(state->'usage'->>'memory')::bigint;
  SELECT customer_id INTO customer FROM webdock_admin.projects WHERE id=w.project_id;
  INSERT INTO webdock_auth.workload_usage(plugin_id,unit,customer_id,project_id,hour,samples,cpu_sum,cpu_max,memory_max)
   VALUES(plugin,workload,customer,w.project_id,date_trunc('hour',statement_timestamp()),1,cpu,cpu,memory)
   ON CONFLICT (plugin_id,unit,hour) DO UPDATE SET samples=webdock_auth.workload_usage.samples+1,
    cpu_sum=webdock_auth.workload_usage.cpu_sum+EXCLUDED.cpu_sum, cpu_max=greatest(webdock_auth.workload_usage.cpu_max,EXCLUDED.cpu_max),
    memory_max=greatest(webdock_auth.workload_usage.memory_max,EXCLUDED.memory_max);
  DELETE FROM webdock_auth.workload_usage WHERE plugin_id=plugin AND unit=workload AND hour<statement_timestamp()-interval '35 days';
 END IF;
 RETURN true;
END $$;

-- A resource's lines, newest first, before `before`, containing `search` (case-insensitive), at most 500.
CREATE FUNCTION webdock_rls.resource_logs(project text, plugin text, resource text, search text, before timestamptz) RETURNS jsonb
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE customer text;
BEGIN
 SELECT customer_id INTO customer FROM webdock_admin.projects WHERE id=project;
 IF customer IS NULL OR NOT webdock_rls.customer_allowed(customer,true) THEN RAISE EXCEPTION 'customer access required' USING ERRCODE='42501'; END IF;
 RETURN coalesce((SELECT jsonb_agg(jsonb_build_object('at',l.at,'unit',l.unit,'line',l.line) ORDER BY l.at DESC) FROM (
   SELECT * FROM webdock_auth.workload_log
   WHERE project_id=project AND plugin_id=plugin AND resource_id=resource AND at<coalesce(before,'infinity')
    AND (coalesce(search,'')='' OR strpos(lower(line),lower(search))>0)
   ORDER BY at DESC LIMIT 500) l),'[]'::jsonb);
END $$;
ALTER FUNCTION webdock_rls.resource_logs(text,text,text,text,timestamptz) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.resource_logs(text,text,text,text,timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.resource_logs(text,text,text,text,timestamptz) TO webdock_auth_runtime;
