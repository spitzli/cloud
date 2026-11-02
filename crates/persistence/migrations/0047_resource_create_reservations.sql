-- Count durable create admissions, including revoked/expired invocations and uncertain jobs.
-- The original invocation owns the slot; idempotent retries point to it across generations.
CREATE OR REPLACE FUNCTION webdock_auth.allocated_of(customer text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
 SELECT jsonb_build_object('projects',(SELECT count(*) FROM webdock_admin.projects WHERE customer_id=customer))
  || coalesce((SELECT jsonb_object_agg(m.key,m.total) FROM (SELECT e.key,sum(e.value::bigint) AS total
     FROM webdock_auth.plugin_workload w, jsonb_each_text(w.allocation) e
     WHERE w.customer_id=customer GROUP BY e.key) m), '{}'::jsonb)
  || coalesce((SELECT jsonb_object_agg(r.plugin_id||'.'||r.offering_id,r.n) FROM (
     SELECT plugin_id,offering_id,count(*) AS n FROM (
      SELECT plugin_id,offering_id FROM webdock_auth.plugin_resource WHERE customer_id=customer
      UNION ALL
      SELECT plugin_id,session_context->'resourceCreate'->>'offering' FROM webdock_auth.plugin_invocation
       WHERE customer_id=customer AND kind='action'
        AND session_context->'resourceCreate'->>'invocationID'=id::text
        AND session_context->'resourceCreate'->>'state'='pending'
     ) counted GROUP BY 1,2) r), '{}'::jsonb)
$$;
CREATE INDEX plugin_invocation_pending_resource ON webdock_auth.plugin_invocation(customer_id)
 WHERE kind='action' AND session_context->'resourceCreate'->>'invocationID'=id::text
  AND session_context->'resourceCreate'->>'state'='pending';

-- NULL permits first dispatch; retries return public metadata/job ID or a conservative conflict.
CREATE FUNCTION webdock_rls.reserve_plugin_resource(invocation uuid) RETURNS jsonb
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE v webdock_auth.plugin_invocation%ROWTYPE; prior webdock_auth.plugin_invocation%ROWTYPE;
 checked jsonb; offering text; allowed bigint; used bigint; replay jsonb;
BEGIN
 IF NOT webdock_rls.worker(ARRAY['plugin-controller']::text[]) THEN RAISE EXCEPTION 'plugin controller required' USING ERRCODE='42501'; END IF;
 SELECT * INTO v FROM webdock_auth.plugin_invocation WHERE id=invocation AND kind='action' AND revoked_at IS NULL;
 IF NOT FOUND OR v.session_context->'resourceAction'->>'effect' IS DISTINCT FROM 'create' THEN
  RAISE EXCEPTION 'create invocation required' USING ERRCODE='42501';
 END IF;
 offering:=v.session_context->'resourceAction'->>'offering';
 IF offering IS NULL OR NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_runtime rt
  JOIN webdock_auth.plugin_release r ON r.plugin_id=rt.plugin_id AND r.version=rt.version,
  jsonb_array_elements(r.manifest_bytes::jsonb->'actions') a
  WHERE rt.plugin_id=v.plugin_id AND rt.generation=v.runtime_generation
   AND a->>'name'=v.name AND a->>'resource'='create' AND a->>'offering'=offering) THEN
  RAISE EXCEPTION 'create offering required' USING ERRCODE='42501';
 END IF;
 checked:=webdock_rls.project_limits(v.project_id);
 IF checked->>'customer' IS DISTINCT FROM v.customer_id THEN RAISE EXCEPTION 'resource owner changed' USING ERRCODE='42501'; END IF;
 SELECT * INTO prior FROM webdock_auth.plugin_invocation p WHERE p.kind='action'
  AND p.plugin_id=v.plugin_id AND p.customer_id=v.customer_id AND p.project_id=v.project_id
  AND p.actor_id=v.actor_id AND p.operation_id=v.operation_id AND p.name=v.name
  AND p.session_context->'resourceCreate'->>'invocationID'=p.id::text;
 IF FOUND THEN
  IF prior.request_hash IS DISTINCT FROM v.request_hash OR prior.session_context->'resourceCreate'->>'offering' IS DISTINCT FROM offering THEN
   RETURN '{"conflict":true}'::jsonb;
  END IF;
  IF prior.session_context->'resourceCreate'->>'state'='indexed' AND NOT EXISTS(
   SELECT 1 FROM webdock_auth.plugin_resource WHERE plugin_id=prior.plugin_id
    AND resource_id=prior.session_context->'resourceCreate'->>'resourceID'
    AND customer_id=prior.customer_id AND project_id=prior.project_id AND offering_id=offering) THEN
   RETURN '{"conflict":true}'::jsonb;
  END IF;
  IF prior.id=v.id THEN RETURN NULL; END IF;
  UPDATE webdock_auth.plugin_invocation SET session_context=jsonb_set(session_context,'{resourceCreate}',
   jsonb_build_object('invocationID',prior.id,'offering',offering)) WHERE id=v.id;
  -- Retain only Core-owned public metadata, never the plugin's arbitrary output or credentials.
  SELECT jsonb_build_object('id',id) INTO replay FROM webdock_auth.plugin_job_execution
   WHERE provenance #>> '{session_context,resourceCreate,invocationID}'=prior.id::text;
  IF replay IS NULL AND prior.session_context->'resourceCreate'->>'state'='indexed' THEN
   SELECT jsonb_build_object('resource',jsonb_build_object('id',resource_id,'name',name)) INTO replay
    FROM webdock_auth.plugin_resource WHERE plugin_id=prior.plugin_id
     AND resource_id=prior.session_context->'resourceCreate'->>'resourceID'
     AND customer_id=prior.customer_id AND project_id=prior.project_id AND offering_id=offering;
  END IF;
  RETURN CASE WHEN replay IS NULL THEN '{"conflict":true}'::jsonb ELSE jsonb_build_object('replay',replay) END;
 END IF;
 allowed:=(checked->'limits'->>(v.plugin_id||'.'||offering))::bigint;
 used:=coalesce((checked->'allocated'->>(v.plugin_id||'.'||offering))::bigint,0);
 IF allowed IS NOT NULL AND used>=allowed THEN RETURN checked; END IF;
 UPDATE webdock_auth.plugin_invocation SET session_context=jsonb_set(session_context,'{resourceCreate}',
  jsonb_build_object('invocationID',v.id,'offering',offering,'state','pending')) WHERE id=v.id;
 RETURN NULL;
END $$;
ALTER FUNCTION webdock_rls.reserve_plugin_resource(uuid) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.reserve_plugin_resource(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.reserve_plugin_resource(uuid) TO webdock_worker_runtime;

-- A create can delegate its one resource to exactly one durable job, never fan out the slot.
CREATE UNIQUE INDEX plugin_job_create_reservation ON webdock_auth.plugin_job_execution
 ((provenance #>> '{session_context,resourceCreate,invocationID}'))
 WHERE provenance #>> '{session_context,resourceCreate,invocationID}' IS NOT NULL;

-- A delete's single job is the deletion receipt; bookkeeping fan-out is unsupported.
CREATE UNIQUE INDEX plugin_job_delete_operation ON webdock_auth.plugin_job_execution
 (plugin_id,customer_id,project_id,(provenance->>'actor_id'),(provenance->>'operation_id'))
 WHERE provenance #>> '{session_context,resourceAction,effect}'='delete';

CREATE OR REPLACE FUNCTION webdock_rls.plugin_job_enqueue(job_id text,job_name text,idem text,payload jsonb,maximum integer,fingerprint text) RETURNS jsonb
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE scope jsonb; v webdock_auth.plugin_invocation%ROWTYPE; existing webdock_auth.plugin_job_execution%ROWTYPE;
 reservation webdock_auth.plugin_invocation%ROWTYPE; root text;
BEGIN
 IF NOT webdock_rls.lock_plugin_invocation() THEN RETURN NULL; END IF;
 scope:=webdock_rls.plugin_context('plugin.jobs.enqueue');
 IF scope IS NULL THEN RETURN NULL; END IF;
 SELECT * INTO v FROM webdock_auth.plugin_invocation WHERE id::text=webdock_rls.request_scope()->>'pluginInvocationID' AND kind='action' AND intent='write';
 IF NOT FOUND OR v.session_context->'authority' IS NULL OR v.session_context->'authority'='null'::jsonb
 OR NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_installation p WHERE p.id=v.plugin_id AND p.state='enabled')
 OR NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_grant g WHERE g.plugin_id=v.plugin_id AND g.customer_id=v.customer_id AND g.project_id=v.project_id AND g.jobs ? job_name)
 OR job_id !~ '^job-[a-f0-9]{64}$' OR job_name !~ '^[a-z][a-z0-9_.-]{0,63}$' OR idem !~ '^[A-Za-z0-9_.-]{8,128}$' OR maximum NOT BETWEEN 1 AND 5
 OR fingerprint !~ '^[a-f0-9]{64}$' OR octet_length(payload::text)>32768*128 THEN RETURN NULL; END IF;
 root:=v.session_context->'resourceCreate'->>'invocationID';
 IF v.session_context->'resourceAction'->>'effect'='create' THEN
  SELECT * INTO reservation FROM webdock_auth.plugin_invocation r WHERE r.id::text=root
   AND r.kind='action' AND r.plugin_id=v.plugin_id AND r.customer_id=v.customer_id AND r.project_id=v.project_id
   AND r.actor_id=v.actor_id AND r.operation_id=v.operation_id AND r.request_hash=v.request_hash
   AND r.session_context->'resourceCreate'->>'invocationID'=r.id::text FOR SHARE;
  IF NOT FOUND THEN RETURN NULL; END IF;
 ELSIF v.session_context->'resourceAction'->>'effect'='delete' THEN
  PERFORM resource_id FROM webdock_auth.plugin_resource WHERE plugin_id=v.plugin_id
   AND customer_id=v.customer_id AND project_id=v.project_id AND resource_id=v.session_context->>'resourceID' FOR SHARE;
  IF NOT FOUND THEN RETURN NULL; END IF;
 END IF;
 -- Exact retained acknowledgements remain valid after the reservation is indexed.
 SELECT * INTO existing FROM webdock_auth.plugin_job_execution WHERE plugin_id=v.plugin_id AND customer_id=v.customer_id AND project_id=v.project_id AND idempotency_key=idem;
 IF NOT FOUND THEN
  IF v.session_context->'resourceAction'->>'effect'='create'
   AND reservation.session_context->'resourceCreate'->>'state'<>'pending' THEN RETURN NULL; END IF;
  INSERT INTO webdock_auth.plugin_job_execution(id,plugin_id,customer_id,project_id,name,idempotency_key,payload,grant_revision,runtime_generation,provenance,request_hash,max_attempts)
  VALUES(job_id,v.plugin_id,v.customer_id,v.project_id,job_name,idem,payload,v.grant_revision,v.runtime_generation,to_jsonb(v),fingerprint,maximum) ON CONFLICT DO NOTHING;
  SELECT * INTO existing FROM webdock_auth.plugin_job_execution WHERE plugin_id=v.plugin_id AND customer_id=v.customer_id AND project_id=v.project_id AND idempotency_key=idem;
  IF NOT FOUND THEN RETURN '{"conflict":true}'::jsonb; END IF;
 END IF;
 IF existing.request_hash IS DISTINCT FROM fingerprint
  OR existing.provenance #>> '{session_context,resourceCreate,invocationID}' IS DISTINCT FROM root THEN RETURN '{"conflict":true}'::jsonb; END IF;
 IF v.session_context->'resourceAction'->>'effect' IN ('create','delete') AND (
  existing.provenance->>'operation_id' IS DISTINCT FROM v.operation_id
  OR existing.provenance->>'actor_id' IS DISTINCT FROM v.actor_id
  OR existing.provenance->>'request_hash' IS DISTINCT FROM v.request_hash
  OR existing.provenance->'session_context'->'resourceAction' IS DISTINCT FROM v.session_context->'resourceAction'
  OR existing.provenance #>> '{session_context,resourceID}' IS DISTINCT FROM v.session_context->>'resourceID') THEN RETURN '{"conflict":true}'::jsonb; END IF;
 RETURN jsonb_build_object('id',existing.id);
END $$;

-- Settle only the exact invocation/job. Index insertion and pending->indexed share the limits lock.
CREATE OR REPLACE FUNCTION webdock_rls.record_plugin_resource(invocation uuid, resource text, resource_name text, by_operator boolean) RETURNS boolean
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE v webdock_auth.plugin_invocation%ROWTYPE; source jsonb; action jsonb;
 reservation webdock_auth.plugin_invocation%ROWTYPE; j webdock_auth.plugin_job_execution%ROWTYPE; bound_job text;
BEGIN
 IF NOT webdock_rls.worker(ARRAY['plugin-controller']::text[]) THEN RAISE EXCEPTION 'plugin controller required' USING ERRCODE='42501'; END IF;
 SELECT * INTO v FROM webdock_auth.plugin_invocation WHERE id=invocation AND revoked_at IS NULL;
 IF NOT FOUND THEN RETURN false; END IF;
 source:=to_jsonb(v);
 IF v.kind='job' THEN
  SELECT * INTO j FROM webdock_auth.plugin_job_execution WHERE id=v.job_execution_id AND invocation_id=v.id
   AND claim_token=v.job_claim_token AND status='running' AND NOT cancel_requested AND lease_until>clock_timestamp();
  IF NOT FOUND OR j.plugin_id<>v.plugin_id OR j.customer_id<>v.customer_id OR j.project_id<>v.project_id THEN RETURN false; END IF;
  source:=j.provenance;
 END IF;
 action:=source->'session_context'->'resourceAction';
 -- Compatibility for already admitted actions: resolve their pinned runtime release, never current.
 IF action IS NULL THEN
  SELECT jsonb_build_object('effect',a->'resource','offering',a->'offering') INTO action
   FROM webdock_auth.plugin_runtime rt JOIN webdock_auth.plugin_release r ON r.plugin_id=rt.plugin_id AND r.version=rt.version,
   jsonb_array_elements(r.manifest_bytes::jsonb->'actions') a
   WHERE rt.plugin_id=v.plugin_id AND rt.generation=v.runtime_generation AND a->>'name'=source->>'name';
 END IF;
 IF action->>'effect'='create' THEN
  PERFORM pg_advisory_xact_lock(hashtext('webdock-limits:'||v.customer_id));
  SELECT * INTO reservation FROM webdock_auth.plugin_invocation r
   WHERE r.id::text=source #>> '{session_context,resourceCreate,invocationID}' AND r.kind='action'
    AND r.plugin_id=v.plugin_id AND r.customer_id=v.customer_id AND r.project_id=v.project_id
    AND r.actor_id=source->>'actor_id' AND r.operation_id=source->>'operation_id' AND r.request_hash=source->>'request_hash'
    AND r.name=source->>'name'
    AND r.session_context->'resourceCreate'->>'invocationID'=r.id::text
    AND r.session_context->'resourceCreate'->>'offering'=action->>'offering' FOR UPDATE;
  IF NOT FOUND THEN RETURN false; END IF;
  IF reservation.session_context->'resourceCreate'->>'state'='indexed' THEN
   RETURN reservation.session_context->'resourceCreate'->>'resourceID'=resource AND EXISTS(
    SELECT 1 FROM webdock_auth.plugin_resource WHERE plugin_id=v.plugin_id AND resource_id=resource
     AND customer_id=v.customer_id AND project_id=v.project_id AND offering_id=action->>'offering');
  END IF;
  SELECT id INTO bound_job FROM webdock_auth.plugin_job_execution
   WHERE provenance #>> '{session_context,resourceCreate,invocationID}'=reservation.id::text;
  IF (v.kind='job' AND bound_job IS DISTINCT FROM v.job_execution_id) OR (v.kind='action' AND bound_job IS NOT NULL) THEN RETURN false; END IF;
  IF reservation.session_context->'resourceCreate'->>'state'<>'pending' THEN RETURN false; END IF;
  INSERT INTO webdock_auth.plugin_resource(plugin_id,resource_id,offering_id,customer_id,project_id,name,created_via,created_by)
   VALUES(v.plugin_id,resource,action->>'offering',v.customer_id,v.project_id,resource_name,
    CASE WHEN source->'session_context'->'operator'='true'::jsonb THEN 'operator' ELSE 'self-service' END,source->>'actor_id')
   ON CONFLICT (plugin_id,resource_id) DO NOTHING;
  IF NOT FOUND THEN RETURN false; END IF;
  UPDATE webdock_auth.plugin_invocation SET session_context=jsonb_set(session_context,'{resourceCreate}',
   session_context->'resourceCreate'||jsonb_build_object('state','indexed','resourceID',resource)) WHERE id=reservation.id;
  RETURN true;
 ELSIF action->>'effect'='delete' THEN
  IF resource IS DISTINCT FROM source->'session_context'->>'resourceID' THEN RETURN false; END IF;
  PERFORM pg_advisory_xact_lock(hashtext('webdock-limits:'||v.customer_id));
  PERFORM resource_id FROM webdock_auth.plugin_resource WHERE plugin_id=v.plugin_id AND resource_id=resource
   AND project_id=v.project_id AND customer_id=v.customer_id FOR UPDATE;
  SELECT id INTO bound_job FROM webdock_auth.plugin_job_execution e
   WHERE e.plugin_id=v.plugin_id AND e.customer_id=v.customer_id AND e.project_id=v.project_id
    AND e.provenance->>'actor_id'=source->>'actor_id'
    AND e.provenance->>'operation_id'=source->>'operation_id'
    AND e.provenance #>> '{session_context,resourceAction,effect}'='delete';
  IF (v.kind='action' AND bound_job IS NOT NULL)
   OR (v.kind='job' AND (bound_job IS DISTINCT FROM v.job_execution_id OR NOT EXISTS(
    SELECT 1 FROM webdock_auth.plugin_invocation a WHERE a.id::text=source->>'id'
     AND a.session_context->>'resourceDeleteJob'=v.job_execution_id))) THEN RETURN false; END IF;
  DELETE FROM webdock_auth.plugin_resource WHERE plugin_id=v.plugin_id AND resource_id=resource
   AND project_id=v.project_id AND customer_id=v.customer_id;
  RETURN true;
 END IF;
 RETURN false;
END $$;
