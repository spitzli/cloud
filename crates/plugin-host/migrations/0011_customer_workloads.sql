-- Native0040: retained units may belong to a customer independently of any project.
ALTER TABLE webdock_auth.plugin_workload ADD COLUMN customer_id text
 REFERENCES webdock_admin.customers(id) ON DELETE RESTRICT;
UPDATE webdock_auth.plugin_workload w SET customer_id=p.customer_id
 FROM webdock_admin.projects p WHERE p.id=w.project_id;
ALTER TABLE webdock_auth.plugin_workload ALTER COLUMN customer_id SET NOT NULL,
 ALTER COLUMN project_id DROP NOT NULL,
 ADD CONSTRAINT plugin_workload_customer_resource CHECK (project_id IS NOT NULL OR resource_id IS NULL);
CREATE INDEX plugin_workload_customer ON webdock_auth.plugin_workload(customer_id);
ALTER TABLE webdock_auth.workload_usage ALTER COLUMN project_id DROP NOT NULL;
ALTER TABLE webdock_auth.notification ALTER COLUMN project_id DROP NOT NULL;
ALTER TABLE webdock_auth.workload_log ALTER COLUMN project_id DROP NOT NULL;

-- Keep ownership and placement immutable even for direct controller writes. Existing project
-- writers may omit customer_id; it is derived from the same project and checked on every insert.
CREATE FUNCTION webdock_auth.guard_workload_owner() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE customer text;
BEGIN
 IF TG_OP='UPDATE' AND (NEW.customer_id IS DISTINCT FROM OLD.customer_id
  OR NEW.project_id IS DISTINCT FROM OLD.project_id OR NEW.cluster_id IS DISTINCT FROM OLD.cluster_id
  OR NEW.namespace IS DISTINCT FROM OLD.namespace) THEN
  RAISE EXCEPTION 'Workload ownership and cluster cannot change' USING ERRCODE='23514';
 END IF;
 IF NEW.project_id IS NOT NULL THEN
  SELECT customer_id INTO customer FROM webdock_admin.projects WHERE id=NEW.project_id;
  IF customer IS NULL THEN RAISE EXCEPTION 'Unknown workload project' USING ERRCODE='23503'; END IF;
  IF NEW.customer_id IS NULL AND TG_OP='INSERT' THEN NEW.customer_id:=customer; END IF;
  IF NEW.customer_id IS DISTINCT FROM customer THEN
   RAISE EXCEPTION 'Workload project/customer mismatch' USING ERRCODE='23514';
  END IF;
 END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_auth.guard_workload_owner() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.guard_workload_owner() FROM PUBLIC;
CREATE TRIGGER guard_workload_owner BEFORE INSERT OR UPDATE ON webdock_auth.plugin_workload
 FOR EACH ROW EXECUTE FUNCTION webdock_auth.guard_workload_owner();

CREATE FUNCTION webdock_auth.guard_workload_project_customer() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 IF NEW.customer_id IS DISTINCT FROM OLD.customer_id AND EXISTS(
  SELECT 1 FROM webdock_auth.plugin_workload w WHERE w.project_id=OLD.id) THEN
  RAISE EXCEPTION 'A project with retained workloads cannot change customer' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_auth.guard_workload_project_customer() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.guard_workload_project_customer() FROM PUBLIC;
CREATE TRIGGER guard_workload_project_customer BEFORE UPDATE OF customer_id ON webdock_admin.projects
 FOR EACH ROW EXECUTE FUNCTION webdock_auth.guard_workload_project_customer();

-- Locks project placement, active customer ownership and the shared customer allocation budget.
-- A customer unit never looks up a project, including a default or a first project.
CREATE FUNCTION webdock_rls.workload_owner(project text, customer text) RETURNS jsonb
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE owner text; cluster text;
BEGIN
 IF NOT webdock_rls.worker(ARRAY['plugin-controller']::text[]) THEN RAISE EXCEPTION 'plugin controller required' USING ERRCODE='42501'; END IF;
 IF (project IS NULL)=(customer IS NULL) THEN RAISE EXCEPTION 'one workload owner required' USING ERRCODE='22023'; END IF;
 IF project IS NOT NULL THEN
  SELECT p.customer_id,p.cluster_id INTO owner,cluster FROM webdock_admin.projects p
   WHERE p.id=project AND p.status='active'
    AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=p.id AND d.status<>'deleted')
   FOR UPDATE OF p;
 ELSE owner:=customer;
 END IF;
 IF owner IS NULL OR NOT EXISTS(SELECT 1 FROM webdock_admin.customers c WHERE c.id=owner AND c.status='active' FOR SHARE) THEN
  RAISE EXCEPTION 'active workload owner required' USING ERRCODE='42501';
 END IF;
 PERFORM pg_advisory_xact_lock(hashtext('webdock-limits:'||owner));
 RETURN jsonb_build_object('customer',owner,'cluster',cluster,'limits',webdock_auth.limits_of(owner),'allocated',webdock_auth.allocated_of(owner));
END $$;
ALTER FUNCTION webdock_rls.workload_owner(text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.workload_owner(text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.workload_owner(text,text) TO webdock_worker_runtime;

CREATE FUNCTION webdock_rls.customer_workload_cluster(customer text, cluster text) RETURNS boolean
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 IF NOT webdock_rls.worker(ARRAY['plugin-controller']::text[]) THEN RAISE EXCEPTION 'plugin controller required' USING ERRCODE='42501'; END IF;
 RETURN EXISTS(SELECT 1 FROM webdock_auth.hosting_cluster c WHERE c.id=cluster AND c.verified
  AND (c.dedicated_customer_id IS NULL OR c.dedicated_customer_id=customer) FOR SHARE);
END $$;
ALTER FUNCTION webdock_rls.customer_workload_cluster(text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.customer_workload_cluster(text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.customer_workload_cluster(text,text) TO webdock_worker_runtime;

CREATE FUNCTION webdock_rls.customer_unverified_host(customer text, hosts text[]) RETURNS text
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 IF NOT webdock_rls.worker(ARRAY['plugin-controller']::text[]) THEN RAISE EXCEPTION 'plugin controller required' USING ERRCODE='42501'; END IF;
 RETURN (SELECT h FROM unnest(hosts) WITH ORDINALITY AS t(h,n)
  WHERE NOT EXISTS(SELECT 1 FROM webdock_auth.customer_domain d WHERE d.customer_id=customer
   AND d.verified_at IS NOT NULL AND (t.h=d.domain OR right(t.h,char_length(d.domain)+1)='.'||d.domain)) ORDER BY n LIMIT 1);
END $$;
ALTER FUNCTION webdock_rls.customer_unverified_host(text,text[]) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.customer_unverified_host(text,text[]) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.customer_unverified_host(text,text[]) TO webdock_worker_runtime;

CREATE OR REPLACE FUNCTION webdock_auth.allocated_of(customer text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
 SELECT jsonb_build_object('projects',(SELECT count(*) FROM webdock_admin.projects WHERE customer_id=customer))
  || coalesce((SELECT jsonb_object_agg(m.key,m.total) FROM (SELECT e.key,sum(e.value::bigint) AS total
     FROM webdock_auth.plugin_workload w, jsonb_each_text(w.allocation) e
     WHERE w.customer_id=customer GROUP BY e.key) m), '{}'::jsonb)
  || coalesce((SELECT jsonb_object_agg(r.plugin_id||'.'||r.offering_id,r.n) FROM (SELECT plugin_id,offering_id,count(*) AS n
     FROM webdock_auth.plugin_resource WHERE customer_id=customer GROUP BY 1,2) r), '{}'::jsonb)
$$;

CREATE OR REPLACE FUNCTION webdock_rls.customer_usage(customer text) RETURNS jsonb
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 IF customer IS NULL OR NOT webdock_rls.customer_allowed(customer) THEN RAISE EXCEPTION 'customer access required' USING ERRCODE='42501'; END IF;
 RETURN jsonb_build_object(
  'plan',(SELECT jsonb_build_object('id',p.id,'name',p.name,'description',p.description,'assigned',a.plan_id IS NOT NULL)
          FROM webdock_auth.plan p LEFT JOIN webdock_auth.customer_plan a ON a.customer_id=customer
          WHERE p.id=coalesce(a.plan_id,(SELECT id FROM webdock_auth.plan WHERE is_default))),
  'overrides',coalesce((SELECT overrides FROM webdock_auth.customer_plan WHERE customer_id=customer),'{}'::jsonb),
  'limits',webdock_auth.limits_of(customer),
  'allocated',webdock_auth.allocated_of(customer),
  'units',coalesce((SELECT jsonb_agg(jsonb_build_object('plugin',w.plugin_id,'unit',w.unit,
     'project',CASE WHEN p.id IS NOT NULL THEN jsonb_build_object('id',p.id,'name',p.name) END,
     'resource',CASE WHEN r.resource_id IS NOT NULL THEN jsonb_build_object('id',r.resource_id,'name',r.name,'offering',r.offering_id) END,
     'allocation',w.allocation,'usage',w.observed->'usage','observedAt',w.observed_at) ORDER BY p.name,w.plugin_id,w.unit)
   FROM webdock_auth.plugin_workload w LEFT JOIN webdock_admin.projects p ON p.id=w.project_id AND p.customer_id=w.customer_id
   LEFT JOIN webdock_auth.plugin_resource r ON r.plugin_id=w.plugin_id AND r.resource_id=w.resource_id AND r.project_id=w.project_id AND r.customer_id=w.customer_id
   WHERE w.customer_id=customer AND NOT w.removing),'[]'::jsonb),
  'history',coalesce((SELECT jsonb_agg(jsonb_build_object('hour',h.hour,'cpu',h.cpu,'memory',h.memory) ORDER BY h.hour)
   FROM (SELECT hour,sum(cpu_sum/samples) AS cpu,sum(memory_max) AS memory FROM webdock_auth.workload_usage
         WHERE customer_id=customer AND hour>=date_trunc('hour',statement_timestamp())-interval '7 days' GROUP BY hour) h),'[]'::jsonb));
END $$;

CREATE OR REPLACE FUNCTION webdock_auth.notice(w webdock_auth.plugin_workload, kinds text[]) RETURNS void
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 UPDATE webdock_auth.notification SET resolved_at=statement_timestamp()
  WHERE plugin_id=w.plugin_id AND unit=w.unit AND resolved_at IS NULL AND NOT kind=ANY(kinds);
 IF cardinality(kinds)=0 THEN RETURN; END IF;
 INSERT INTO webdock_auth.notification(customer_id,project_id,plugin_id,unit,resource_id,kind)
  SELECT w.customer_id,w.project_id,w.plugin_id,w.unit,w.resource_id,k FROM unnest(kinds) k
  ON CONFLICT (plugin_id,unit,kind) WHERE resolved_at IS NULL DO NOTHING;
END $$;

CREATE OR REPLACE FUNCTION webdock_rls.customer_notifications(customer text) RETURNS jsonb
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 IF customer IS NULL OR NOT webdock_rls.customer_allowed(customer) THEN RAISE EXCEPTION 'customer access required' USING ERRCODE='42501'; END IF;
 RETURN coalesce((SELECT jsonb_agg(jsonb_build_object('id',n.id::text,'kind',n.kind,'plugin',n.plugin_id,'unit',n.unit,
   'project',CASE WHEN p.id IS NOT NULL THEN jsonb_build_object('id',p.id,'name',p.name) END,
   'resource',CASE WHEN r.resource_id IS NOT NULL THEN jsonb_build_object('id',r.resource_id,'name',r.name) END,
   'openedAt',n.opened_at,'resolvedAt',n.resolved_at) ORDER BY n.resolved_at IS NOT NULL, n.opened_at DESC)
  FROM webdock_auth.notification n LEFT JOIN webdock_admin.projects p ON p.id=n.project_id AND p.customer_id=n.customer_id
  LEFT JOIN webdock_auth.plugin_resource r ON r.plugin_id=n.plugin_id AND r.resource_id=n.resource_id AND r.project_id=n.project_id AND r.customer_id=n.customer_id
  WHERE n.customer_id=customer AND coalesce(n.resolved_at,statement_timestamp())-n.opened_at>=webdock_auth.notice_delay(n.kind)
   AND (n.resolved_at IS NULL OR n.resolved_at>statement_timestamp()-interval '7 days')),'[]'::jsonb);
END $$;

CREATE OR REPLACE FUNCTION webdock_rls.domain_users(customer text, target text) RETURNS jsonb
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 IF NOT webdock_rls.customer_allowed(customer) THEN RAISE EXCEPTION 'customer access required' USING ERRCODE='42501'; END IF;
 RETURN coalesce((SELECT jsonb_agg(jsonb_build_object('plugin',w.plugin_id,'unit',w.unit,'project',p.name,
   'resource',(SELECT r.name FROM webdock_auth.plugin_resource r WHERE r.plugin_id=w.plugin_id AND r.resource_id=w.resource_id AND r.project_id=w.project_id AND r.customer_id=w.customer_id)) ORDER BY p.name,w.unit)
  FROM webdock_auth.plugin_workload w LEFT JOIN webdock_admin.projects p ON p.id=w.project_id AND p.customer_id=w.customer_id
  WHERE w.customer_id=customer AND NOT w.removing
   AND EXISTS(SELECT 1 FROM unnest(w.hosts) h WHERE h=target OR right(h,char_length(target)+1)='.'||target)),'[]'::jsonb);
END $$;

-- Only the current revision can change observations, history or pruning/removal state.
CREATE OR REPLACE FUNCTION webdock_rls.report_plugin_workload(plugin text, workload text, reported bigint, state jsonb, failure text, removed boolean) RETURNS boolean
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE w webdock_auth.plugin_workload; cpu bigint; memory bigint;
BEGIN
 SELECT * INTO w FROM webdock_auth.plugin_workload WHERE plugin_id=plugin AND unit=workload FOR UPDATE;
 IF NOT FOUND OR NOT webdock_rls.worker(ARRAY['agent']::text[], w.cluster_id::text) OR reported<>w.revision OR reported<1 THEN RETURN false; END IF;
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
  INSERT INTO webdock_auth.workload_usage(plugin_id,unit,customer_id,project_id,hour,samples,cpu_sum,cpu_max,memory_max)
   VALUES(plugin,workload,w.customer_id,w.project_id,date_trunc('hour',statement_timestamp()),1,cpu,cpu,memory)
   ON CONFLICT (plugin_id,unit,hour) DO UPDATE SET samples=webdock_auth.workload_usage.samples+1,
    cpu_sum=webdock_auth.workload_usage.cpu_sum+EXCLUDED.cpu_sum, cpu_max=greatest(webdock_auth.workload_usage.cpu_max,EXCLUDED.cpu_max),
    memory_max=greatest(webdock_auth.workload_usage.memory_max,EXCLUDED.memory_max);
  DELETE FROM webdock_auth.workload_usage WHERE plugin_id=plugin AND unit=workload AND hour<statement_timestamp()-interval '35 days';
 END IF;
 RETURN true;
END $$;
