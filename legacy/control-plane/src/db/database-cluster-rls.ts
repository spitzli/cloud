/** Redacted infrastructure view for admission; retains row locks without exposing agent credentials. */
export const databaseClusterRlsSQL = String.raw`
CREATE OR REPLACE FUNCTION webdock_admin.lock_hosting_project(p_project_id varchar) RETURNS void
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 IF NOT webdock_rls.project_allowed(p_project_id) AND NOT webdock_rls.worker(ARRAY['git-worker','database-gateway']) AND NOT webdock_rls.agent_project(p_project_id) THEN
  RAISE EXCEPTION 'Project authorization required' USING ERRCODE='42501';
 END IF;
 PERFORM id FROM webdock_admin.projects WHERE id=p_project_id FOR SHARE;
END $$;
ALTER FUNCTION webdock_admin.lock_hosting_project(varchar) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_admin.lock_hosting_project(varchar) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_admin.lock_hosting_project(varchar) TO webdock_auth_runtime,webdock_worker_runtime;
CREATE OR REPLACE FUNCTION webdock_rls.cluster_snapshot(cluster_id text) RETURNS jsonb
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE c record; a record; observation_value jsonb; pools jsonb;
BEGIN
 IF webdock_rls.capacity_cluster_allowed(cluster_id) IS NOT TRUE THEN RAISE EXCEPTION 'Cluster authorization required' USING ERRCODE='42501'; END IF;
 SELECT * INTO c FROM webdock_auth.hosting_cluster x WHERE x.id=cluster_snapshot.cluster_id FOR SHARE;
 IF NOT FOUND THEN RETURN NULL; END IF;
 SELECT revoked,last_seen,observation INTO a FROM webdock_auth.hosting_agent x WHERE x.cluster_id=cluster_snapshot.cluster_id FOR SHARE;
 observation_value:=a.observation;
 IF jsonb_typeof(observation_value#>'{capabilities,databasePools}')='array' THEN
  SELECT coalesce(jsonb_agg(value-'allocations'-'observedDatabases'),'[]'::jsonb) INTO pools FROM jsonb_array_elements(observation_value#>'{capabilities,databasePools}');
  observation_value:=jsonb_set(observation_value,'{capabilities,databasePools}',pools);
 END IF;
 RETURN jsonb_build_object('id',c.id,'name',c.name,'ownership',c.ownership,'provider',c.provider,'country',c.country,'region',c.region,
  'locationEvidence',CASE WHEN webdock_rls.operator() THEN c.location_evidence ELSE NULL END,'dedicatedCustomerID',c.dedicated_customer_id,
  'revision',c.revision,'verified',c.verified,'capacity',c.capacity,'verificationEvidence',CASE WHEN webdock_rls.operator() THEN c.verification_evidence ELSE NULL END,
  'revoked',a.revoked,'lastSeen',a.last_seen,'last_seen',a.last_seen,'observation',observation_value);
END $$;
ALTER FUNCTION webdock_rls.cluster_snapshot(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.cluster_snapshot(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.cluster_snapshot(text) TO webdock_auth_runtime,webdock_worker_runtime;
`;
