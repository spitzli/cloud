-- Native0028: plugin workloads. Core stores the objects a plugin wants on a cluster, pinned to a
-- Core-chosen namespace; that cluster's enrolled agent applies them and reports what it observed.
-- Plugins never reach this table; agents only read their cluster's rows and report through
-- webdock_rls.report_plugin_workload.
CREATE TABLE webdock_auth.plugin_workload (
 plugin_id text NOT NULL REFERENCES webdock_auth.plugin_installation(id) ON DELETE RESTRICT,
 unit text NOT NULL CHECK (unit ~ '^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$'),
 project_id varchar NOT NULL REFERENCES webdock_admin.projects(id) ON DELETE RESTRICT,
 cluster_id varchar NOT NULL REFERENCES webdock_auth.hosting_cluster(id) ON DELETE RESTRICT,
 namespace text NOT NULL CHECK (namespace ~ '^wd-[a-z0-9-]{1,60}$'),
 objects jsonb NOT NULL CHECK (jsonb_typeof(objects)='array'),
 removing boolean NOT NULL DEFAULT false,
 revision bigint NOT NULL DEFAULT 1 CHECK (revision>0),
 applied_revision bigint NOT NULL DEFAULT 0 CHECK (applied_revision>=0 AND applied_revision<=revision),
 observed jsonb CHECK (observed IS NULL OR jsonb_typeof(observed)='object'),
 error text CHECK (char_length(error)<=500),
 observed_at timestamptz,
 updated_at timestamptz NOT NULL DEFAULT now(),
 PRIMARY KEY (plugin_id, unit)
);
CREATE INDEX plugin_workload_cluster ON webdock_auth.plugin_workload(cluster_id, observed_at NULLS FIRST);
ALTER TABLE webdock_auth.plugin_workload OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.plugin_workload ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.plugin_workload FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_auth.plugin_workload FROM PUBLIC;
GRANT SELECT, INSERT, UPDATE, DELETE ON webdock_auth.plugin_workload TO webdock_worker_runtime;
CREATE POLICY platform_owner ON webdock_auth.plugin_workload TO webdock_platform_owner USING (true) WITH CHECK (true);
CREATE POLICY plugin_workload_controller ON webdock_auth.plugin_workload TO webdock_worker_runtime
 USING (webdock_rls.worker(ARRAY['plugin-controller']::text[])) WITH CHECK (webdock_rls.worker(ARRAY['plugin-controller']::text[]));
CREATE POLICY plugin_workload_agent_read ON webdock_auth.plugin_workload FOR SELECT TO webdock_worker_runtime
 USING (webdock_rls.worker(ARRAY['agent']::text[], cluster_id::text));

-- The only agent write: observation of one revision, or the end of a removal.
CREATE FUNCTION webdock_rls.report_plugin_workload(plugin text, workload text, reported bigint, state jsonb, failure text, removed boolean) RETURNS boolean
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE w webdock_auth.plugin_workload;
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
 RETURN true;
END $$;
ALTER FUNCTION webdock_rls.report_plugin_workload(text,text,bigint,jsonb,text,boolean) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.report_plugin_workload(text,text,bigint,jsonb,text,boolean) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.report_plugin_workload(text,text,bigint,jsonb,text,boolean) TO webdock_worker_runtime;
