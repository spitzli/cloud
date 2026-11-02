-- Native0030: plans, limits and usage. A plan is a named set of limits ({meter: integer}; a missing
-- meter is unlimited). A customer has at most one assigned plan plus overrides (null lifts a limit);
-- without an assignment the default plan applies, and without a default plan nothing is limited.
-- Core counts projects and resources and sums what pinned workloads allocate; the cluster's agent
-- reports what they actually use, folded into hourly samples.
-- A fresh schema has no snowflake state yet, so the Default project trigger had no id to give.
INSERT INTO webdock_admin.snowflake_state(singleton) SELECT true WHERE NOT EXISTS(SELECT 1 FROM webdock_admin.snowflake_state);

CREATE FUNCTION webdock_auth.valid_limits(value jsonb, nullable boolean) RETURNS boolean
LANGUAGE sql IMMUTABLE SET search_path=pg_catalog AS $$
 SELECT jsonb_typeof(value)='object' AND (SELECT count(*) FROM jsonb_object_keys(value))<=64
  AND NOT EXISTS(SELECT 1 FROM jsonb_each(value) e WHERE e.key !~ '^[a-z][a-z0-9-]{0,63}(\.[a-z][a-z0-9-]{0,63})?$'
   OR NOT ((nullable AND e.value='null'::jsonb) OR (jsonb_typeof(e.value)='number' AND e.value::text ~ '^[0-9]{1,15}$')))
$$;

CREATE TABLE webdock_auth.plan (
 id text PRIMARY KEY CHECK (id ~ '^[1-9][0-9]{0,18}$'),
 name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80),
 description text NOT NULL DEFAULT '' CHECK (char_length(description)<=500),
 limits jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (webdock_auth.valid_limits(limits,false)),
 is_default boolean NOT NULL DEFAULT false,
 updated_by text NOT NULL,
 updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX plan_default ON webdock_auth.plan(is_default) WHERE is_default;
CREATE TABLE webdock_auth.customer_plan (
 customer_id text PRIMARY KEY REFERENCES webdock_admin.customers(id) ON DELETE CASCADE,
 plan_id text REFERENCES webdock_auth.plan(id) ON DELETE RESTRICT,
 overrides jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (webdock_auth.valid_limits(overrides,true)),
 updated_by text NOT NULL,
 updated_at timestamptz NOT NULL DEFAULT now()
);
DO $$
DECLARE t text;
BEGIN
 FOREACH t IN ARRAY ARRAY['plan','customer_plan'] LOOP
  EXECUTE format('ALTER TABLE webdock_auth.%I OWNER TO webdock_platform_owner', t);
  EXECUTE format('ALTER TABLE webdock_auth.%I ENABLE ROW LEVEL SECURITY', t);
  EXECUTE format('ALTER TABLE webdock_auth.%I FORCE ROW LEVEL SECURITY', t);
  EXECUTE format('REVOKE ALL ON TABLE webdock_auth.%I FROM PUBLIC', t);
  EXECUTE format('CREATE POLICY platform_owner ON webdock_auth.%I TO webdock_platform_owner USING (true) WITH CHECK (true)', t);
  EXECUTE format('CREATE POLICY operator_manage ON webdock_auth.%I TO webdock_auth_runtime USING (webdock_rls.operator()) WITH CHECK (webdock_rls.operator())', t);
  EXECUTE format('GRANT SELECT,INSERT,UPDATE,DELETE ON TABLE webdock_auth.%I TO webdock_auth_runtime', t);
 END LOOP;
END $$;

-- What a unit's objects allocate ({memory MiB, cpu millicores, storage MiB}), computed by Core when
-- it pins them, and the plugin resource the unit belongs to (shown only within the same project).
ALTER TABLE webdock_auth.plugin_workload
 ADD COLUMN allocation jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (webdock_auth.valid_limits(allocation,false)),
 ADD COLUMN resource_id text CHECK (resource_id ~ '^[A-Za-z0-9_.:-]{1,128}$');

-- Hourly samples of what each unit used: CPU in millicores, memory in MiB. Kept 35 days.
CREATE TABLE webdock_auth.workload_usage (
 plugin_id text NOT NULL,
 unit text NOT NULL,
 customer_id text NOT NULL REFERENCES webdock_admin.customers(id) ON DELETE CASCADE,
 project_id text NOT NULL,
 hour timestamptz NOT NULL,
 samples integer NOT NULL CHECK (samples>0),
 cpu_sum bigint NOT NULL CHECK (cpu_sum>=0),
 cpu_max bigint NOT NULL CHECK (cpu_max>=0),
 memory_max bigint NOT NULL CHECK (memory_max>=0),
 PRIMARY KEY (plugin_id, unit, hour)
);
CREATE INDEX workload_usage_customer ON webdock_auth.workload_usage(customer_id, hour);
ALTER TABLE webdock_auth.workload_usage OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.workload_usage ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.workload_usage FORCE ROW LEVEL SECURITY;
REVOKE ALL ON TABLE webdock_auth.workload_usage FROM PUBLIC;
CREATE POLICY platform_owner ON webdock_auth.workload_usage TO webdock_platform_owner USING (true) WITH CHECK (true);

-- Unchecked internals; only the functions below and the triggers call them.
CREATE FUNCTION webdock_auth.limits_of(customer text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
 SELECT jsonb_strip_nulls(
  coalesce((SELECT p.limits FROM webdock_auth.customer_plan c JOIN webdock_auth.plan p ON p.id=c.plan_id WHERE c.customer_id=customer),
           (SELECT limits FROM webdock_auth.plan WHERE is_default), '{}'::jsonb)
  || coalesce((SELECT overrides FROM webdock_auth.customer_plan WHERE customer_id=customer), '{}'::jsonb))
$$;
CREATE FUNCTION webdock_auth.allocated_of(customer text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
 SELECT jsonb_build_object('projects',(SELECT count(*) FROM webdock_admin.projects WHERE customer_id=customer))
  || coalesce((SELECT jsonb_object_agg(m.key,m.total) FROM (SELECT e.key,sum(e.value::bigint) AS total
     FROM webdock_auth.plugin_workload w JOIN webdock_admin.projects p ON p.id=w.project_id, jsonb_each_text(w.allocation) e
     WHERE p.customer_id=customer GROUP BY e.key) m), '{}'::jsonb)
  || coalesce((SELECT jsonb_object_agg(r.plugin_id||'.'||r.offering_id,r.n) FROM (SELECT plugin_id,offering_id,count(*) AS n
     FROM webdock_auth.plugin_resource WHERE customer_id=customer GROUP BY 1,2) r), '{}'::jsonb)
$$;
DO $$
DECLARE f text;
BEGIN
 FOREACH f IN ARRAY ARRAY['limits_of(text)','allocated_of(text)','valid_limits(jsonb,boolean)'] LOOP
  EXECUTE format('ALTER FUNCTION webdock_auth.%s OWNER TO webdock_platform_owner', f);
  EXECUTE format('REVOKE ALL ON FUNCTION webdock_auth.%s FROM PUBLIC', f);
 END LOOP;
END $$;
GRANT EXECUTE ON FUNCTION webdock_auth.valid_limits(jsonb,boolean) TO webdock_auth_runtime, webdock_worker_runtime;

-- The plugin controller checks limits before it admits a create or pins a workload in a project.
-- Serialised per customer so two workloads cannot both take the last free memory.
CREATE FUNCTION webdock_rls.project_limits(project text) RETURNS jsonb
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE customer text;
BEGIN
 IF NOT webdock_rls.worker(ARRAY['plugin-controller']::text[]) THEN RAISE EXCEPTION 'plugin controller required' USING ERRCODE='42501'; END IF;
 SELECT customer_id INTO customer FROM webdock_admin.projects WHERE id=project;
 IF customer IS NULL THEN RAISE EXCEPTION 'unknown project' USING ERRCODE='P0002'; END IF;
 PERFORM pg_advisory_xact_lock(hashtext('webdock-limits:'||customer));
 RETURN jsonb_build_object('customer',customer,'limits',webdock_auth.limits_of(customer),'allocated',webdock_auth.allocated_of(customer));
END $$;
ALTER FUNCTION webdock_rls.project_limits(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.project_limits(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.project_limits(text) TO webdock_worker_runtime;

-- A customer's plan, limits, what is allocated, each unit with its measured use, and 7 days of
-- hourly totals. Members of the customer and operators may read it.
CREATE FUNCTION webdock_rls.customer_usage(customer text) RETURNS jsonb
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
     'project',jsonb_build_object('id',p.id,'name',p.name),
     'resource',CASE WHEN r.resource_id IS NOT NULL THEN jsonb_build_object('id',r.resource_id,'name',r.name,'offering',r.offering_id) END,
     'allocation',w.allocation,'usage',w.observed->'usage','observedAt',w.observed_at) ORDER BY p.name,w.plugin_id,w.unit)
   FROM webdock_auth.plugin_workload w JOIN webdock_admin.projects p ON p.id=w.project_id
   LEFT JOIN webdock_auth.plugin_resource r ON r.plugin_id=w.plugin_id AND r.resource_id=w.resource_id AND r.project_id=w.project_id
   WHERE p.customer_id=customer AND NOT w.removing),'[]'::jsonb),
  'history',coalesce((SELECT jsonb_agg(jsonb_build_object('hour',h.hour,'cpu',h.cpu,'memory',h.memory) ORDER BY h.hour)
   FROM (SELECT hour,sum(cpu_sum/samples) AS cpu,sum(memory_max) AS memory FROM webdock_auth.workload_usage
         WHERE customer_id=customer AND hour>=date_trunc('hour',statement_timestamp())-interval '7 days' GROUP BY hour) h),'[]'::jsonb));
END $$;
ALTER FUNCTION webdock_rls.customer_usage(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.customer_usage(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.customer_usage(text) TO webdock_auth_runtime;

-- Projects count against the plan; the Default project every customer starts with always exists.
CREATE FUNCTION webdock_auth.limit_projects() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE allowed bigint;
BEGIN
 IF NEW.created_default THEN RETURN NEW; END IF;
 PERFORM pg_advisory_xact_lock(hashtext('webdock-limits:'||NEW.customer_id));
 allowed:=(webdock_auth.limits_of(NEW.customer_id)->>'projects')::bigint;
 IF allowed IS NOT NULL AND (SELECT count(*) FROM webdock_admin.projects WHERE customer_id=NEW.customer_id)>=allowed THEN
  RAISE EXCEPTION 'The plan allows % projects.', allowed USING ERRCODE='WD429';
 END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_auth.limit_projects() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.limit_projects() FROM PUBLIC;
CREATE TRIGGER webdock_limit_projects BEFORE INSERT ON webdock_admin.projects
 FOR EACH ROW EXECUTE FUNCTION webdock_auth.limit_projects();

-- The agent's report also folds the unit's measured use into its hourly sample.
CREATE OR REPLACE FUNCTION webdock_rls.report_plugin_workload(plugin text, workload text, reported bigint, state jsonb, failure text, removed boolean) RETURNS boolean
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE w webdock_auth.plugin_workload; cpu bigint; memory bigint; customer text;
BEGIN
 SELECT * INTO w FROM webdock_auth.plugin_workload WHERE plugin_id=plugin AND unit=workload FOR UPDATE;
 IF NOT FOUND OR NOT webdock_rls.worker(ARRAY['agent']::text[], w.cluster_id::text) OR reported>w.revision OR reported<1 THEN RETURN false; END IF;
 IF removed THEN
  IF NOT w.removing OR reported<>w.revision THEN RETURN false; END IF;
  DELETE FROM webdock_auth.plugin_workload WHERE plugin_id=plugin AND unit=workload;
  RETURN true;
 END IF;
 IF state IS NULL OR jsonb_typeof(state)<>'object' OR octet_length(state::text)>65536 THEN RETURN false; END IF;
 UPDATE webdock_auth.plugin_workload SET applied_revision=greatest(applied_revision,reported), observed=state,
  error=left(failure,500), observed_at=statement_timestamp() WHERE plugin_id=plugin AND unit=workload;
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
