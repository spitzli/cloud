-- Preserve immutable migration54. Disambiguate its PL/pgSQL record w from a SQL alias.
CREATE OR REPLACE FUNCTION webdock_rls.recovery_admit_source_fence(operation text,generation integer,token_hash text,expected_epoch uuid,participants jsonb,event_id text,audit_id text) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE j webdock_auth.hosting_recovery_job%ROWTYPE;p webdock_auth.hosting_recovery_policy%ROWTYPE;i webdock_auth.plugin_installation%ROWTYPE;w webdock_auth.plugin_workload%ROWTYPE;fenced jsonb;part jsonb;seen text[]:='{}';plan_hash text;existing webdock_auth.hosting_recovery_source_fence%ROWTYPE;
BEGIN
 IF webdock_rls.recovery_source_lease(operation,generation,token_hash,expected_epoch) IS NOT TRUE
  OR jsonb_typeof(participants)<>'array' OR jsonb_array_length(participants)<1 OR jsonb_array_length(participants)>200 OR octet_length(participants::text)>65536 THEN
  RAISE EXCEPTION 'Current source-fence lease and participant inventory required' USING ERRCODE='42501';END IF;
 SELECT * INTO j FROM webdock_auth.hosting_recovery_job WHERE operation_id=operation;
 SELECT * INTO p FROM webdock_auth.hosting_recovery_policy WHERE id=j.policy_id;
 SELECT * INTO i FROM webdock_auth.plugin_installation WHERE id='mail';
 -- No capture while a preexisting installation transition can still deploy a writer.
 IF EXISTS(SELECT 1 FROM webdock_auth.plugin_operation WHERE plugin_id='mail' AND status IN('pending','running','blocked')) THEN
  RAISE EXCEPTION 'Mail installation transition is not drained' USING ERRCODE='55000';END IF;
 IF EXISTS(SELECT 1 FROM webdock_mail.operation WHERE state IN('pending','running','needs_review'))
  OR EXISTS(SELECT 1 FROM webdock_mail.configuration_operation WHERE state IN('pending','running','needs_review')) THEN
  RAISE EXCEPTION 'Native Mail lifecycle/configuration work is not drained' USING ERRCODE='55000';END IF;
 FOR part IN SELECT value FROM jsonb_array_elements(participants) LOOP
  IF jsonb_typeof(part)<>'object' OR coalesce(part->>'kind','') NOT IN('guarded-agent','legacy-worker','runtime','ce')
    OR coalesce(length(part->>'identity'),0) NOT BETWEEN 1 AND 256 OR part->>'identity'=ANY(seen)
    OR coalesce(part->>'ownerSha256','') !~ '^[a-f0-9]{64}$' OR coalesce(length(part->>'uid'),0) NOT BETWEEN 1 AND 256 THEN
    RAISE EXCEPTION 'Exact owned source participant required' USING ERRCODE='42501';END IF;
  seen:=array_append(seen,part->>'identity');
  IF part->>'kind'='guarded-agent' AND ((part->>'agentGeneration')::integer IS DISTINCT FROM j.agent_generation OR part->>'effectGuardVersion' IS DISTINCT FROM '3') THEN
   RAISE EXCEPTION 'Current guarded executor required' USING ERRCODE='42501';END IF;
 END LOOP;
 IF j.agent_identity IS NULL OR NOT EXISTS(SELECT 1 FROM jsonb_array_elements(participants) v WHERE v->>'kind'='guarded-agent' AND v->>'identity'=j.agent_identity->>'identity' AND v->>'uid'=j.agent_identity->>'uid') THEN
  RAISE EXCEPTION 'Authenticated recovery executor identity required' USING ERRCODE='42501';END IF;
 -- Every canonical enrolled executor is accounted for. Other/legacy executors
 -- cannot be marked as guarded merely by claiming the updated capability version.
 FOR part IN SELECT jsonb_build_object('identity','agent:'||a.installation||':'||a.agent_id,'uid',a.credential_hash,'generation',a.generation) FROM webdock_auth.agent_transport_enrollment a WHERE a.cluster_id=p.cluster_id AND NOT a.revoked LOOP
  IF NOT EXISTS(SELECT 1 FROM jsonb_array_elements(participants) v WHERE v->>'identity'=part->>'identity' AND v->>'uid'=part->>'uid'
   AND ((v->>'identity'=j.agent_identity->>'identity' AND v->>'kind'='guarded-agent') OR(v->>'identity'<>j.agent_identity->>'identity' AND v->>'kind'='legacy-worker'))) THEN
   RAISE EXCEPTION 'Known legacy executor is not accounted for' USING ERRCODE='55000';END IF;
 END LOOP;
 FOR part IN SELECT jsonb_build_object('namespace',r.namespace,'generation',r.generation,'image',r.image) FROM webdock_auth.plugin_runtime r WHERE r.plugin_id='mail' AND r.status IN('prepared','starting','ready','draining') LOOP
  IF NOT EXISTS(SELECT 1 FROM jsonb_array_elements(participants) v WHERE v->>'kind'='runtime' AND v->>'namespace'=part->>'namespace' AND v->>'runtimeGeneration'=part->>'generation' AND v->>'image'=part->>'image') THEN
   RAISE EXCEPTION 'Known Mail runtime is not accounted for' USING ERRCODE='55000';END IF;
 END LOOP;
 FOR part IN SELECT jsonb_build_object('namespace',source_workload.namespace,'unit',source_workload.unit) FROM webdock_auth.plugin_workload source_workload WHERE source_workload.plugin_id='mail' LOOP
  IF NOT EXISTS(SELECT 1 FROM jsonb_array_elements(participants) v WHERE v->>'kind'='ce' AND v->>'namespace'=part->>'namespace' AND v->>'unit'=part->>'unit') THEN
   RAISE EXCEPTION 'Known Mail workload is not accounted for' USING ERRCODE='55000';END IF;
 END LOOP;
 FOR part IN SELECT jsonb_build_object('customerID',s.customer_id,'clusterID',s.host_id,'revision',s.revision,'uid',n.checkpoint_runtime_id)
  FROM webdock_mail.service s JOIN LATERAL (SELECT checkpoint_runtime_id FROM webdock_mail.native_operation WHERE customer_id=s.customer_id AND checkpoint_runtime_id IS NOT NULL ORDER BY created_at DESC,operation_id DESC LIMIT 1) n ON true LOOP
  IF part->>'clusterID'<>p.cluster_id OR NOT EXISTS(SELECT 1 FROM jsonb_array_elements(participants) v
   WHERE v->>'kind'='ce' AND v->>'customerID'=part->>'customerID' AND v->>'uid'=part->>'uid'
    AND v->>'serviceRevision'=part->>'revision' AND v->>'native'='true') THEN
   RAISE EXCEPTION 'Known native Mail instance is not accounted for' USING ERRCODE='55000';END IF;
 END LOOP;
 plan_hash:=encode(sha256(convert_to(participants::text,'UTF8')),'hex');
 SELECT * INTO existing FROM webdock_auth.hosting_recovery_source_fence WHERE operation_id=operation AND source_kind='mail' FOR UPDATE;
 IF FOUND THEN
  IF existing.operation_generation<>generation OR existing.agent_generation<>j.agent_generation OR existing.epoch<>expected_epoch OR existing.plan_sha256<>plan_hash THEN
   RAISE EXCEPTION 'Source participant plan changed' USING ERRCODE='42501';END IF;
 ELSE
 -- Core-native history IDs, never checkpoint input.
 IF ((CASE WHEN event_id ~ '^[1-9][0-9]{0,18}$' THEN event_id::numeric<=9223372036854775807 ELSE false END) AND (CASE WHEN audit_id ~ '^[1-9][0-9]{0,18}$' THEN audit_id::numeric<=9223372036854775807 ELSE false END) AND event_id<>audit_id) IS NOT TRUE THEN RAISE EXCEPTION 'Native recovery record IDs required' USING ERRCODE='22023';END IF;
 -- End native history IDs.
  INSERT INTO webdock_auth.hosting_recovery_source_fence(operation_id,policy_id,policy_revision,operation_generation,agent_generation,epoch,installation_generation,installation_revision,plan_sha256,participants,state)
   VALUES(operation,p.id,p.revision,generation,j.agent_generation,expected_epoch,i.generation,i.revision,plan_hash,participants,'draining');
  FOR w IN SELECT * FROM webdock_auth.plugin_workload WHERE plugin_id='mail' FOR UPDATE LOOP
   IF w.cluster_id<>p.cluster_id OR w.removing THEN RAISE EXCEPTION 'Mail workload ownership/transition is not drained' USING ERRCODE='55000';END IF;
   SELECT jsonb_agg(CASE WHEN value->>'kind' IN('Deployment','StatefulSet','ReplicaSet','ReplicationController') THEN jsonb_set(value,'{spec,replicas}','0') WHEN value->>'kind'='CronJob' THEN jsonb_set(value,'{spec,suspend}','true') ELSE value END ORDER BY ordinality)
    INTO fenced FROM jsonb_array_elements(w.objects) WITH ORDINALITY;
   INSERT INTO webdock_auth.hosting_recovery_source_intent(operation_id,plugin_id,unit,cluster_id,namespace,prior_revision,prior_objects,fenced_revision,fenced_objects,prior_sha256,fenced_sha256)
    VALUES(operation,w.plugin_id,w.unit,w.cluster_id,w.namespace,w.revision,w.objects,w.revision+1,fenced,encode(sha256(convert_to(w.objects::text,'UTF8')),'hex'),encode(sha256(convert_to(fenced::text,'UTF8')),'hex'));
   UPDATE webdock_auth.plugin_workload SET objects=fenced,revision=revision+1,updated_at=clock_timestamp() WHERE plugin_id=w.plugin_id AND unit=w.unit AND revision=w.revision AND objects=w.objects;
   IF NOT FOUND THEN RAISE EXCEPTION 'Source intent compare-and-set failed' USING ERRCODE='40001';END IF;
  END LOOP;
  INSERT INTO webdock_auth.hosting_recovery_source_event(id,operation_id,event,proof) VALUES(event_id,operation,'admitted',jsonb_build_object('planSha256',plan_hash,'participantCount',jsonb_array_length(participants),'agentGeneration',j.agent_generation));
  INSERT INTO webdock_auth.hosting_audit(id,subject,action,target_id,outcome) VALUES(audit_id,p.approved_by,'recovery.source-fence',operation,'draining');
 END IF;
 RETURN jsonb_build_object('version',1,'operationID',operation,'state','draining','planSha256',plan_hash,'participants',participants);
END $$;
ALTER FUNCTION webdock_rls.recovery_admit_source_fence(text,integer,text,uuid,jsonb,text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.recovery_admit_source_fence(text,integer,text,uuid,jsonb,text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.recovery_admit_source_fence(text,integer,text,uuid,jsonb,text,text) TO webdock_worker_runtime;
