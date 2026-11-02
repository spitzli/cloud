-- Native0035: placement. A project may name the cluster its workloads run on: a verified cluster the
-- operator offers to everyone, or one dedicated to the project's customer. Without a choice, each
-- plugin's default cluster applies. The choice can change only while nothing runs in the project.
ALTER TABLE webdock_auth.hosting_cluster ADD COLUMN offered boolean NOT NULL DEFAULT false;
ALTER TABLE webdock_admin.projects ADD COLUMN cluster_id varchar REFERENCES webdock_auth.hosting_cluster(id) ON DELETE RESTRICT;

-- Clusters this customer's projects may run on.
CREATE FUNCTION webdock_auth.usable_clusters(customer text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
 SELECT coalesce(jsonb_agg(jsonb_build_object('id',c.id,'name',c.name,'region',c.region,'country',c.country,'dedicated',c.dedicated_customer_id IS NOT NULL) ORDER BY c.name),'[]'::jsonb)
 FROM webdock_auth.hosting_cluster c
 WHERE c.verified AND ((c.offered AND c.dedicated_customer_id IS NULL) OR c.dedicated_customer_id=customer)
$$;
ALTER FUNCTION webdock_auth.usable_clusters(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.usable_clusters(text) FROM PUBLIC;

-- A project's cluster and the choices, for its members.
CREATE FUNCTION webdock_rls.project_placement(project text) RETURNS jsonb
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE p webdock_admin.projects;
BEGIN
 SELECT * INTO p FROM webdock_admin.projects WHERE id=project;
 IF NOT FOUND OR NOT webdock_rls.customer_allowed(p.customer_id) THEN RAISE EXCEPTION 'customer access required' USING ERRCODE='42501'; END IF;
 RETURN jsonb_build_object('cluster',p.cluster_id,'options',webdock_auth.usable_clusters(p.customer_id),
  'movable',NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_workload w WHERE w.project_id=project));
END $$;
ALTER FUNCTION webdock_rls.project_placement(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.project_placement(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.project_placement(text) TO webdock_auth_runtime;

-- Owners, admins and operators choose; NULL goes back to the plugins' defaults.
CREATE FUNCTION webdock_rls.set_project_cluster(project text, cluster text) RETURNS void
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE p webdock_admin.projects;
BEGIN
 SELECT * INTO p FROM webdock_admin.projects WHERE id=project FOR UPDATE;
 IF NOT FOUND OR NOT webdock_rls.customer_allowed(p.customer_id,true) THEN RAISE EXCEPTION 'customer access required' USING ERRCODE='42501'; END IF;
 IF cluster IS NOT NULL AND NOT EXISTS(SELECT 1 FROM jsonb_array_elements(webdock_auth.usable_clusters(p.customer_id)) c WHERE c->>'id'=cluster) THEN
  RAISE EXCEPTION 'This cluster is not available to this workspace.' USING ERRCODE='WD404';
 END IF;
 IF p.cluster_id IS DISTINCT FROM cluster AND EXISTS(SELECT 1 FROM webdock_auth.plugin_workload w WHERE w.project_id=project) THEN
  RAISE EXCEPTION 'Services already run in this project; it stays on its cluster.' USING ERRCODE='WD409';
 END IF;
 UPDATE webdock_admin.projects SET cluster_id=cluster WHERE id=project;
END $$;
ALTER FUNCTION webdock_rls.set_project_cluster(text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.set_project_cluster(text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.set_project_cluster(text,text) TO webdock_auth_runtime;

-- Where the plugin controller places a project's new units, if the project chose.
CREATE FUNCTION webdock_rls.project_cluster(project text) RETURNS text
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 IF NOT webdock_rls.worker(ARRAY['plugin-controller']::text[]) THEN RAISE EXCEPTION 'plugin controller required' USING ERRCODE='42501'; END IF;
 RETURN (SELECT cluster_id FROM webdock_admin.projects WHERE id=project);
END $$;
ALTER FUNCTION webdock_rls.project_cluster(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.project_cluster(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.project_cluster(text) TO webdock_worker_runtime;

-- Operators list clusters and choose which they offer.
CREATE FUNCTION webdock_rls.operator_clusters() RETURNS jsonb
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 IF NOT webdock_rls.operator() THEN RAISE EXCEPTION 'operator required' USING ERRCODE='42501'; END IF;
 RETURN coalesce((SELECT jsonb_agg(jsonb_build_object('id',c.id,'name',c.name,'region',c.region,'country',c.country,'verified',c.verified,'offered',c.offered,
   'dedicatedTo',(SELECT jsonb_build_object('id',k.id,'name',k.name) FROM webdock_admin.customers k WHERE k.id=c.dedicated_customer_id),
   'projects',(SELECT count(*) FROM webdock_admin.projects p WHERE p.cluster_id=c.id)) ORDER BY c.name) FROM webdock_auth.hosting_cluster c),'[]'::jsonb);
END $$;
ALTER FUNCTION webdock_rls.operator_clusters() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.operator_clusters() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.operator_clusters() TO webdock_auth_runtime;
CREATE FUNCTION webdock_rls.offer_cluster(cluster text, offer boolean) RETURNS boolean
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 IF NOT webdock_rls.operator() THEN RAISE EXCEPTION 'operator required' USING ERRCODE='42501'; END IF;
 UPDATE webdock_auth.hosting_cluster SET offered=offer WHERE id=cluster;
 RETURN FOUND;
END $$;
ALTER FUNCTION webdock_rls.offer_cluster(text,boolean) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.offer_cluster(text,boolean) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.offer_cluster(text,boolean) TO webdock_auth_runtime;
