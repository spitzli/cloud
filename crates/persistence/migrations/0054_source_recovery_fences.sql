-- Forward source quiescence custody. No send state is duplicated here.
ALTER TABLE webdock_auth.hosting_recovery_job ADD COLUMN agent_identity jsonb;
CREATE TABLE webdock_auth.hosting_recovery_source_fence (
 operation_id varchar PRIMARY KEY REFERENCES webdock_auth.hosting_recovery_job(operation_id),
 policy_id varchar NOT NULL REFERENCES webdock_auth.hosting_recovery_policy(id),
 policy_revision integer NOT NULL, operation_generation integer NOT NULL,
 agent_generation integer NOT NULL, epoch uuid NOT NULL,
 source_kind text NOT NULL DEFAULT 'mail' CHECK(source_kind IN('mail','app')),
 installation_generation bigint, installation_revision bigint,
 source_app_id varchar REFERENCES webdock_auth.hosting_app(id), source_app_revision integer,
 source_observed_uid text, source_spec_sha256 text, source_environment_sha256 text,
 source_operation_id varchar REFERENCES webdock_auth.hosting_operation(id),
 source_operation_generation integer, source_operation_status text, source_operation_fingerprint text,
 plan_sha256 text NOT NULL CHECK(plan_sha256~'^[a-f0-9]{64}$'),
 participants jsonb NOT NULL CHECK(jsonb_typeof(participants)='array' AND octet_length(participants::text)<=65536),
 state text NOT NULL CHECK(state IN('draining','drained','resumed','uncertain','stopped','captured','resuming','released')),
 created_at timestamptz NOT NULL DEFAULT now(),
 CHECK(coalesce((source_kind='mail' AND installation_generation IS NOT NULL AND installation_revision IS NOT NULL
   AND source_app_id IS NULL AND source_app_revision IS NULL AND source_observed_uid IS NULL
   AND source_spec_sha256 IS NULL AND source_environment_sha256 IS NULL AND source_operation_id IS NULL
   AND source_operation_generation IS NULL AND source_operation_status IS NULL AND source_operation_fingerprint IS NULL
   AND state IN('draining','drained','resumed','uncertain'))
  OR (source_kind='app' AND installation_generation IS NULL AND installation_revision IS NULL
   AND source_app_id IS NOT NULL AND source_app_revision>0 AND source_observed_uid IS NOT NULL
   AND length(source_observed_uid) BETWEEN 1 AND 256 AND source_spec_sha256~'^[a-f0-9]{64}$'
   AND source_environment_sha256~'^[a-f0-9]{64}$' AND source_operation_id IS NOT NULL
   AND source_operation_generation>0 AND source_operation_status='succeeded'
   AND source_operation_fingerprint~'^[a-f0-9]{64}$'
   AND state IN('draining','stopped','captured','resuming','released','uncertain')),false))
);
CREATE UNIQUE INDEX recovery_one_closed_app_source ON webdock_auth.hosting_recovery_source_fence(source_app_id)
 WHERE source_kind='app' AND state<>'released';
CREATE TABLE webdock_auth.hosting_recovery_source_intent (
 operation_id varchar NOT NULL REFERENCES webdock_auth.hosting_recovery_source_fence(operation_id),
 plugin_id text NOT NULL CHECK(plugin_id='mail'), unit text NOT NULL,
 cluster_id varchar NOT NULL, namespace text NOT NULL,
 prior_revision bigint NOT NULL, prior_objects jsonb NOT NULL,
 fenced_revision bigint NOT NULL, fenced_objects jsonb NOT NULL,
 prior_sha256 text NOT NULL, fenced_sha256 text NOT NULL,
 PRIMARY KEY(operation_id,plugin_id,unit),CHECK(fenced_revision=prior_revision+1)
);
CREATE TABLE webdock_auth.hosting_recovery_source_event (
 id varchar PRIMARY KEY,
 operation_id varchar NOT NULL REFERENCES webdock_auth.hosting_recovery_source_fence(operation_id),
 event text NOT NULL, proof jsonb NOT NULL CHECK(octet_length(proof::text)<=65536),
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE FUNCTION webdock_rls.recovery_source_immutable() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 IF TG_TABLE_NAME<>'hosting_recovery_source_fence' OR TG_OP='DELETE' THEN
  RAISE EXCEPTION 'Immutable source-fence custody' USING ERRCODE='42501';END IF;
 IF (NEW.operation_id,NEW.policy_id,NEW.policy_revision,NEW.operation_generation,NEW.agent_generation,NEW.epoch,NEW.installation_generation,NEW.installation_revision,NEW.plan_sha256,NEW.participants,NEW.created_at,NEW.source_kind,NEW.source_app_id,NEW.source_app_revision,NEW.source_observed_uid,NEW.source_spec_sha256,NEW.source_environment_sha256,NEW.source_operation_id,NEW.source_operation_generation,NEW.source_operation_status,NEW.source_operation_fingerprint)
  IS DISTINCT FROM (OLD.operation_id,OLD.policy_id,OLD.policy_revision,OLD.operation_generation,OLD.agent_generation,OLD.epoch,OLD.installation_generation,OLD.installation_revision,OLD.plan_sha256,OLD.participants,OLD.created_at,OLD.source_kind,OLD.source_app_id,OLD.source_app_revision,OLD.source_observed_uid,OLD.source_spec_sha256,OLD.source_environment_sha256,OLD.source_operation_id,OLD.source_operation_generation,OLD.source_operation_status,OLD.source_operation_fingerprint)
  OR current_user<>'webdock_platform_owner' THEN RAISE EXCEPTION 'Immutable source-fence grant' USING ERRCODE='42501'; END IF;
 RETURN NEW;
END $$;
DO $$ DECLARE t text; BEGIN
 FOREACH t IN ARRAY ARRAY['hosting_recovery_source_fence','hosting_recovery_source_intent','hosting_recovery_source_event'] LOOP
  EXECUTE format('ALTER TABLE webdock_auth.%I OWNER TO webdock_platform_owner',t);
  EXECUTE format('ALTER TABLE webdock_auth.%I ENABLE ROW LEVEL SECURITY',t);
  EXECUTE format('ALTER TABLE webdock_auth.%I FORCE ROW LEVEL SECURITY',t);
  EXECUTE format('REVOKE ALL ON webdock_auth.%I FROM PUBLIC,webdock_auth_runtime,webdock_worker_runtime',t);
  EXECUTE format('CREATE POLICY platform_owner ON webdock_auth.%I TO webdock_platform_owner USING(true) WITH CHECK(true)',t);
  EXECUTE format('CREATE TRIGGER source_custody_immutable BEFORE UPDATE OR DELETE ON webdock_auth.%I FOR EACH ROW EXECUTE FUNCTION webdock_rls.recovery_source_immutable()',t);
 END LOOP;
END $$;

CREATE FUNCTION webdock_rls.recovery_source_lease(operation text,generation integer,token_hash text,expected_epoch uuid) RETURNS boolean
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE j webdock_auth.hosting_recovery_job%ROWTYPE;p webdock_auth.hosting_recovery_policy%ROWTYPE;o webdock_auth.hosting_operation%ROWTYPE;a webdock_auth.hosting_agent%ROWTYPE;e webdock_auth.native_recovery_epoch%ROWTYPE;i webdock_auth.plugin_installation%ROWTYPE;
BEGIN
 SELECT * INTO j FROM webdock_auth.hosting_recovery_job WHERE operation_id=operation FOR SHARE;
 IF NOT FOUND THEN RETURN false; END IF;
 SELECT * INTO p FROM webdock_auth.hosting_recovery_policy WHERE id=j.policy_id FOR SHARE;
 SELECT * INTO o FROM webdock_auth.hosting_operation WHERE id=operation FOR SHARE;
 SELECT * INTO a FROM webdock_auth.hosting_agent WHERE cluster_id=p.cluster_id FOR SHARE;
 SELECT * INTO e FROM webdock_auth.native_recovery_epoch WHERE singleton FOR UPDATE;
 SELECT * INTO i FROM webdock_auth.plugin_installation WHERE id='mail' FOR SHARE;
 RETURN coalesce(webdock_rls.worker(ARRAY['agent'],p.cluster_id)
  AND p.profile='mail-installation-v1' AND webdock_rls.recovery_policy_live(p.id,p.cluster_id)
  AND p.revision=j.policy_revision AND j.agent_generation=a.generation AND NOT a.revoked
  AND j.lease_hash=token_hash AND o.generation=recovery_source_lease.generation
  AND o.status='running' AND o.lease_until>clock_timestamp()
  AND e.epoch=expected_epoch AND e.operation_id=operation AND e.generation=recovery_source_lease.generation
  AND e.intent_revision=p.revision AND (e.state IN('fencing','fenced') OR e.state='active' AND EXISTS(SELECT 1 FROM webdock_auth.hosting_recovery_source_fence f WHERE f.operation_id=operation AND f.source_kind='mail' AND f.state='resumed'))
  AND i.generation=(p.source->>'mailGeneration')::bigint AND i.revision=(p.source->>'mailRevision')::bigint,false);
END $$;
ALTER FUNCTION webdock_rls.recovery_source_lease(text,integer,text,uuid) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.recovery_source_lease(text,integer,text,uuid) FROM PUBLIC;

DROP FUNCTION IF EXISTS webdock_rls.recovery_admit_source_fence(text,integer,text,uuid,jsonb);
CREATE FUNCTION webdock_rls.recovery_admit_source_fence(operation text,generation integer,token_hash text,expected_epoch uuid,participants jsonb,event_id text,audit_id text) RETURNS jsonb
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
 FOR part IN SELECT jsonb_build_object('namespace',w.namespace,'unit',w.unit) FROM webdock_auth.plugin_workload w WHERE w.plugin_id='mail' LOOP
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

DROP FUNCTION IF EXISTS webdock_rls.recovery_mark_source_drained(text,integer,text,uuid,jsonb);
CREATE FUNCTION webdock_rls.recovery_mark_source_drained(operation text,generation integer,token_hash text,expected_epoch uuid,proof jsonb,event_id text) RETURNS boolean
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE f webdock_auth.hosting_recovery_source_fence%ROWTYPE;part jsonb;outcome jsonb;w webdock_auth.hosting_recovery_source_intent%ROWTYPE;
BEGIN
 IF webdock_rls.recovery_source_lease(operation,generation,token_hash,expected_epoch) IS NOT TRUE THEN RAISE EXCEPTION 'Current source drain lease required' USING ERRCODE='42501';END IF;
 SELECT * INTO f FROM webdock_auth.hosting_recovery_source_fence WHERE operation_id=operation AND source_kind='mail' FOR UPDATE;
 IF FOUND AND f.state IN('drained','resumed') THEN
  RETURN EXISTS(SELECT 1 FROM webdock_auth.hosting_recovery_source_event event WHERE event.operation_id=operation AND event.event='drained' AND event.proof=proof);
 END IF;
 IF NOT FOUND OR f.state<>'draining' OR proof->>'planSha256' IS DISTINCT FROM f.plan_sha256
  OR proof->'allPVCWritersAbsent' IS DISTINCT FROM 'true'::jsonb OR jsonb_typeof(proof->'participants')<>'array'
  OR jsonb_array_length(proof->'participants')<>jsonb_array_length(f.participants) THEN RETURN false; END IF;
 FOR part IN SELECT value FROM jsonb_array_elements(f.participants) LOOP
  SELECT value INTO outcome FROM jsonb_array_elements(proof->'participants') WHERE value->>'identity'=part->>'identity';
  IF NOT FOUND OR outcome->>'uid' IS DISTINCT FROM part->>'uid' OR outcome->>'ownerSha256' IS DISTINCT FROM part->>'ownerSha256'
    OR coalesce(outcome->>'state','') NOT IN('stopped','completed','guarded-drained') OR outcome->'uncertain' IS DISTINCT FROM 'false'::jsonb
    OR (part->>'kind'='legacy-worker' AND outcome->>'state' NOT IN('stopped','completed'))
    OR (part->>'kind'='legacy-worker' AND coalesce(outcome->>'providerOutcome','') NOT IN('not-started','confirmed-complete'))
    OR (outcome->>'state'='guarded-drained' AND (part->>'kind'<>'guarded-agent' OR outcome->>'effectGuardVersion' IS DISTINCT FROM '3')) THEN RETURN false; END IF;
 END LOOP;
 FOR w IN SELECT * FROM webdock_auth.hosting_recovery_source_intent WHERE operation_id=operation LOOP
  IF NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_workload current WHERE current.plugin_id=w.plugin_id AND current.unit=w.unit AND current.revision=w.fenced_revision AND current.objects=w.fenced_objects) THEN RETURN false;END IF;
 END LOOP;
 -- Core-native history IDs, never checkpoint input.
 IF ((CASE WHEN event_id ~ '^[1-9][0-9]{0,18}$' THEN event_id::numeric<=9223372036854775807 ELSE false END)) IS NOT TRUE THEN RAISE EXCEPTION 'Native recovery record IDs required' USING ERRCODE='22023';END IF;
 -- End native history IDs.
 UPDATE webdock_auth.hosting_recovery_source_fence SET state='drained' WHERE operation_id=operation AND source_kind='mail';
 INSERT INTO webdock_auth.hosting_recovery_source_event(id,operation_id,event,proof) VALUES(event_id,operation,'drained',proof);
 RETURN true;
END $$;
ALTER FUNCTION webdock_rls.recovery_mark_source_drained(text,integer,text,uuid,jsonb,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.recovery_mark_source_drained(text,integer,text,uuid,jsonb,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.recovery_mark_source_drained(text,integer,text,uuid,jsonb,text) TO webdock_worker_runtime;

-- Keep0052's exact signature, but never acknowledge capture until all known work is drained.
CREATE OR REPLACE FUNCTION webdock_rls.recovery_mail_fence(operation text,generation integer,token_hash text,finish boolean DEFAULT false) RETURNS uuid
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE j webdock_auth.hosting_recovery_job%ROWTYPE;p webdock_auth.hosting_recovery_policy%ROWTYPE;e webdock_auth.native_recovery_epoch%ROWTYPE;
BEGIN
 SELECT * INTO j FROM webdock_auth.hosting_recovery_job WHERE operation_id=operation FOR SHARE;
 SELECT * INTO p FROM webdock_auth.hosting_recovery_policy WHERE id=j.policy_id FOR SHARE;
 IF webdock_rls.worker(ARRAY['agent'],p.cluster_id) IS NOT TRUE OR p.profile IS DISTINCT FROM 'mail-installation-v1' OR webdock_rls.recovery_policy_live(p.id,p.cluster_id) IS NOT TRUE OR j.policy_revision IS DISTINCT FROM p.revision OR j.lease_hash IS DISTINCT FROM token_hash
  OR NOT EXISTS(SELECT 1 FROM webdock_auth.hosting_operation o JOIN webdock_auth.hosting_agent a ON a.cluster_id=o.cluster_id WHERE o.id=operation AND o.status='running' AND o.generation=recovery_mail_fence.generation AND o.lease_until>clock_timestamp() AND a.generation=j.agent_generation AND NOT a.revoked) THEN
  RAISE EXCEPTION 'Current recovery lease required' USING ERRCODE='42501';END IF;
 SELECT * INTO e FROM webdock_auth.native_recovery_epoch WHERE singleton FOR UPDATE;
 IF e.state='quarantined' OR e.state<>'active' AND (e.operation_id,e.generation) IS DISTINCT FROM(operation,generation) THEN RAISE EXCEPTION 'Owned recovery epoch required' USING ERRCODE='42501';END IF;
 IF EXISTS(SELECT 1 FROM webdock_auth.hosting_recovery_source_fence f WHERE f.operation_id=operation AND f.source_kind='mail' AND f.state='resumed' AND f.operation_generation=generation AND f.agent_generation=j.agent_generation AND f.epoch=e.epoch) THEN
  IF e.state<>'active' OR e.operation_id IS DISTINCT FROM operation OR e.generation<>generation THEN RAISE EXCEPTION 'Compensated epoch changed' USING ERRCODE='42501'; END IF;
  RETURN e.epoch;
 END IF;
 IF e.state='active' THEN
  IF (j.spec->>'epoch')::uuid=e.epoch THEN RAISE EXCEPTION 'A new recovery cycle requires a fresh epoch' USING ERRCODE='42501';END IF;
  UPDATE webdock_auth.native_recovery_epoch SET epoch=(j.spec->>'epoch')::uuid,state='fencing',operation_id=operation,generation=recovery_mail_fence.generation,intent_revision=p.revision,source_resume_allowed=true WHERE singleton RETURNING * INTO e;
 END IF;
 IF finish THEN
  IF NOT EXISTS(SELECT 1 FROM webdock_auth.hosting_recovery_source_fence f WHERE f.operation_id=operation AND f.source_kind='mail' AND f.state='drained' AND f.operation_generation=generation AND f.agent_generation=j.agent_generation AND f.epoch=e.epoch) THEN
   RAISE EXCEPTION 'Known source executors are not drained' USING ERRCODE='55000';END IF;
  UPDATE webdock_mail.send_job SET state='unknown',finished_at=NULL WHERE state='handoff';
  UPDATE webdock_auth.native_recovery_epoch SET state='fenced' WHERE singleton;
 END IF;RETURN e.epoch;
END $$;
ALTER FUNCTION webdock_rls.recovery_mail_fence(text,integer,text,boolean) OWNER TO webdock_platform_owner;

DROP FUNCTION IF EXISTS webdock_rls.recovery_source_resume(text,integer,text,uuid);
CREATE FUNCTION webdock_rls.recovery_source_resume(operation text,generation integer,token_hash text,expected_epoch uuid,event_id text,audit_id text) RETURNS boolean
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE f webdock_auth.hosting_recovery_source_fence%ROWTYPE;w webdock_auth.hosting_recovery_source_intent%ROWTYPE;p webdock_auth.hosting_recovery_policy%ROWTYPE;
BEGIN
 IF webdock_rls.recovery_source_lease(operation,generation,token_hash,expected_epoch) IS NOT TRUE THEN RETURN false;END IF;
 SELECT * INTO f FROM webdock_auth.hosting_recovery_source_fence WHERE operation_id=operation AND source_kind='mail' FOR UPDATE;
 IF FOUND AND f.state='resumed' AND f.operation_generation=generation AND f.epoch=expected_epoch THEN RETURN true;END IF;
 IF NOT FOUND OR f.state<>'drained' OR f.operation_generation<>generation OR f.epoch<>expected_epoch THEN RETURN false;END IF;
 -- Core-native history IDs, never checkpoint input.
 IF ((CASE WHEN event_id ~ '^[1-9][0-9]{0,18}$' THEN event_id::numeric<=9223372036854775807 ELSE false END) AND (CASE WHEN audit_id ~ '^[1-9][0-9]{0,18}$' THEN audit_id::numeric<=9223372036854775807 ELSE false END) AND event_id<>audit_id) IS NOT TRUE THEN RAISE EXCEPTION 'Native recovery record IDs required' USING ERRCODE='22023';END IF;
 -- End native history IDs.
 SELECT * INTO p FROM webdock_auth.hosting_recovery_policy WHERE id=f.policy_id;
 FOR w IN SELECT * FROM webdock_auth.hosting_recovery_source_intent WHERE operation_id=operation LOOP
  IF NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_workload current WHERE current.plugin_id=w.plugin_id AND current.unit=w.unit AND current.revision=w.fenced_revision AND current.objects=w.fenced_objects FOR UPDATE) THEN RETURN false;END IF;
 END LOOP;
 FOR w IN SELECT * FROM webdock_auth.hosting_recovery_source_intent WHERE operation_id=operation LOOP
  UPDATE webdock_auth.plugin_workload SET objects=w.prior_objects,revision=revision+1,updated_at=clock_timestamp() WHERE plugin_id=w.plugin_id AND unit=w.unit AND revision=w.fenced_revision AND objects=w.fenced_objects;
  IF NOT FOUND THEN RAISE EXCEPTION 'Source resume compare-and-set failed' USING ERRCODE='40001';END IF;
 END LOOP;
 UPDATE webdock_auth.native_recovery_epoch SET state='active',source_resume_allowed=false WHERE singleton AND state='fenced' AND epoch=expected_epoch AND operation_id=operation AND native_recovery_epoch.generation=recovery_source_resume.generation;
 IF NOT FOUND THEN RAISE EXCEPTION 'Source epoch changed' USING ERRCODE='40001';END IF;
 UPDATE webdock_auth.hosting_recovery_source_fence SET state='resumed' WHERE operation_id=operation AND source_kind='mail';
 INSERT INTO webdock_auth.hosting_recovery_source_event(id,operation_id,event,proof) VALUES(event_id,operation,'resumed',jsonb_build_object('epoch',expected_epoch,'generation',generation));
 INSERT INTO webdock_auth.hosting_audit(id,subject,action,target_id,outcome) VALUES(audit_id,p.approved_by,'recovery.source-compensation',operation,'same-fence-resumed');RETURN true;
END $$;
ALTER FUNCTION webdock_rls.recovery_source_resume(text,integer,text,uuid,text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.recovery_source_resume(text,integer,text,uuid,text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.recovery_source_resume(text,integer,text,uuid,text,text) TO webdock_worker_runtime;

CREATE FUNCTION webdock_rls.recovery_source_fence_view(operation text,generation integer,token_hash text,expected_epoch uuid) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE j webdock_auth.hosting_recovery_job%ROWTYPE;p webdock_auth.hosting_recovery_policy%ROWTYPE;f jsonb;
BEGIN
 SELECT * INTO j FROM webdock_auth.hosting_recovery_job WHERE operation_id=operation;
 SELECT * INTO p FROM webdock_auth.hosting_recovery_policy WHERE id=j.policy_id;
 IF webdock_rls.worker(ARRAY['agent'],p.cluster_id) IS NOT TRUE OR webdock_rls.recovery_policy_live(p.id,p.cluster_id) IS NOT TRUE OR j.lease_hash IS DISTINCT FROM token_hash
  OR NOT EXISTS(SELECT 1 FROM webdock_auth.hosting_operation o JOIN webdock_auth.hosting_agent a ON a.cluster_id=o.cluster_id WHERE o.id=operation AND o.status='running' AND o.generation=recovery_source_fence_view.generation AND o.lease_until>clock_timestamp() AND a.generation=j.agent_generation AND NOT a.revoked) THEN
  RAISE EXCEPTION 'Current recovery source view required' USING ERRCODE='42501';END IF;
 SELECT to_jsonb(s) INTO f FROM webdock_auth.hosting_recovery_source_fence s WHERE s.operation_id=operation AND s.source_kind='mail' AND s.epoch=expected_epoch;
 RETURN jsonb_build_object('version',1,'fence',f,'agentIdentity',j.agent_identity,
  'enrolledExecutors',coalesce((SELECT jsonb_agg(jsonb_build_object('identity','agent:'||a.installation||':'||a.agent_id,'uid',a.credential_hash,'generation',a.generation) ORDER BY a.agent_id) FROM webdock_auth.agent_transport_enrollment a WHERE a.cluster_id=p.cluster_id AND NOT a.revoked),'[]'::jsonb));
END $$;
ALTER FUNCTION webdock_rls.recovery_source_fence_view(text,integer,text,uuid) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.recovery_source_fence_view(text,integer,text,uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.recovery_source_fence_view(text,integer,text,uuid) TO webdock_worker_runtime;

CREATE FUNCTION webdock_rls.recovery_transport_immutable() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 IF OLD.agent_identity IS NOT NULL AND NEW.agent_identity IS DISTINCT FROM OLD.agent_identity THEN
  RAISE EXCEPTION 'Recovery executor custody is immutable' USING ERRCODE='42501';END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_rls.recovery_transport_immutable() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.recovery_transport_immutable() FROM PUBLIC;
CREATE TRIGGER recovery_transport_immutable BEFORE UPDATE ON webdock_auth.hosting_recovery_job
 FOR EACH ROW EXECUTE FUNCTION webdock_rls.recovery_transport_immutable();

CREATE OR REPLACE FUNCTION webdock_rls.recovery_mail_inventory(operation text,generation integer,token_hash text,expected_agent_generation integer) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE j webdock_auth.hosting_recovery_job%ROWTYPE;p webdock_auth.hosting_recovery_policy%ROWTYPE;
BEGIN
 SELECT * INTO j FROM webdock_auth.hosting_recovery_job WHERE operation_id=operation;
 SELECT * INTO p FROM webdock_auth.hosting_recovery_policy WHERE id=j.policy_id;
 IF webdock_rls.worker(ARRAY['agent'],p.cluster_id) IS NOT TRUE OR p.profile IS DISTINCT FROM 'mail-installation-v1' OR webdock_rls.recovery_policy_live(p.id,p.cluster_id) IS NOT TRUE
  OR j.agent_generation IS DISTINCT FROM expected_agent_generation
  OR NOT EXISTS(SELECT 1 FROM webdock_auth.hosting_agent a WHERE a.cluster_id=p.cluster_id AND a.generation=expected_agent_generation AND NOT a.revoked)
  OR j.policy_revision<>p.revision OR j.lease_hash IS DISTINCT FROM token_hash
  OR NOT EXISTS(SELECT 1 FROM webdock_auth.hosting_operation o WHERE o.id=operation AND o.status='running' AND o.generation=recovery_mail_inventory.generation AND o.lease_until>clock_timestamp()) THEN
  RAISE EXCEPTION 'Current recovery lease required' USING ERRCODE='42501'; END IF;
 RETURN jsonb_build_object('version',1,
  'epoch',(SELECT epoch::text FROM webdock_auth.native_recovery_epoch WHERE singleton),
  'services',coalesce((SELECT jsonb_agg(jsonb_build_object('customerID',s.customer_id,'instanceKey',s.instance_key,'hostID',s.host_id,'revision',s.revision,'enabled',s.desired_enabled,'state',s.state,'nativeRuntimeID',(SELECT checkpoint_runtime_id FROM webdock_mail.native_operation n WHERE n.customer_id=s.customer_id AND n.checkpoint_runtime_id IS NOT NULL ORDER BY n.created_at DESC,n.operation_id DESC LIMIT 1)) ORDER BY s.customer_id) FROM webdock_mail.service s),'[]'::jsonb),
  'runtimes',coalesce((SELECT jsonb_agg(jsonb_build_object('pluginID',r.plugin_id,'generation',r.generation,'image',r.image,'clusterID',r.cluster_id,'namespace',r.namespace,'status',r.status) ORDER BY r.generation) FROM webdock_auth.plugin_runtime r WHERE r.plugin_id='mail' AND r.status<>'stopped'),'[]'::jsonb),
  'workloads',coalesce((SELECT jsonb_agg(jsonb_build_object('unit',w.unit,'customerID',w.customer_id,'clusterID',w.cluster_id,'namespace',w.namespace) ORDER BY w.unit) FROM webdock_auth.plugin_workload w WHERE w.plugin_id='mail'),'[]'::jsonb),
  'ledger',jsonb_build_object('jobs',(SELECT count(*) FROM webdock_mail.send_job),'accepted',(SELECT count(*) FROM webdock_mail.send_job WHERE state='accepted'),'unknown',(SELECT count(*) FROM webdock_mail.send_job WHERE state='unknown'),'queued',(SELECT count(*) FROM webdock_mail.send_job WHERE state='queued')));
END $$;
ALTER FUNCTION webdock_rls.recovery_mail_inventory(text,integer,text,integer) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.recovery_mail_inventory(text,integer,text,integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.recovery_mail_inventory(text,integer,text,integer) TO webdock_worker_runtime;

-- Old recovery readers cannot silently discard the authenticated generation binding.
REVOKE EXECUTE ON FUNCTION webdock_rls.recovery_mail_inventory(text,integer,text) FROM webdock_worker_runtime;

-- Exact committed native callbacks may be acknowledged without replaying an effect.
CREATE TABLE webdock_auth.hosting_recovery_callback (
 operation_id varchar NOT NULL REFERENCES webdock_auth.hosting_recovery_job(operation_id),
 generation integer NOT NULL CHECK(generation>0), phase_key text NOT NULL CHECK(length(phase_key) BETWEEN 1 AND 128),
 lease_hash text NOT NULL CHECK(lease_hash~'^[a-f0-9]{64}$'),
 request_sha256 text NOT NULL CHECK(request_sha256~'^[a-f0-9]{64}$'),
 response jsonb NOT NULL CHECK(octet_length(response::text)<=409600),
 created_at timestamptz NOT NULL DEFAULT now(),PRIMARY KEY(operation_id,generation,phase_key)
);
ALTER TABLE webdock_auth.hosting_recovery_callback OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.hosting_recovery_callback ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.hosting_recovery_callback FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_auth.hosting_recovery_callback FROM PUBLIC,webdock_auth_runtime,webdock_worker_runtime;
CREATE POLICY platform_owner ON webdock_auth.hosting_recovery_callback TO webdock_platform_owner USING(true) WITH CHECK(true);
CREATE TRIGGER recovery_callback_immutable BEFORE UPDATE OR DELETE ON webdock_auth.hosting_recovery_callback FOR EACH ROW EXECUTE FUNCTION webdock_rls.recovery_source_immutable();
GRANT SELECT,INSERT ON webdock_auth.hosting_recovery_callback TO webdock_worker_runtime;
CREATE POLICY recovery_callback_read ON webdock_auth.hosting_recovery_callback FOR SELECT TO webdock_worker_runtime USING(
 EXISTS(SELECT 1 FROM webdock_auth.hosting_recovery_job j JOIN webdock_auth.hosting_operation o ON o.id=j.operation_id JOIN webdock_auth.hosting_agent a ON a.cluster_id=o.cluster_id
 WHERE j.operation_id=hosting_recovery_callback.operation_id AND j.agent_generation=a.generation AND NOT a.revoked AND webdock_rls.worker(ARRAY['agent'],o.cluster_id)));
CREATE POLICY recovery_callback_insert ON webdock_auth.hosting_recovery_callback FOR INSERT TO webdock_worker_runtime WITH CHECK(
 EXISTS(SELECT 1 FROM webdock_auth.hosting_recovery_job j JOIN webdock_auth.hosting_operation o ON o.id=j.operation_id JOIN webdock_auth.hosting_agent a ON a.cluster_id=o.cluster_id JOIN webdock_auth.hosting_recovery_policy p ON p.id=j.policy_id
 WHERE j.operation_id=hosting_recovery_callback.operation_id AND j.agent_generation=a.generation AND NOT a.revoked AND o.status='running'
 AND o.generation=hosting_recovery_callback.generation AND o.lease_until>clock_timestamp() AND j.lease_hash=hosting_recovery_callback.lease_hash
 AND p.revision=j.policy_revision AND webdock_rls.worker(ARRAY['agent'],o.cluster_id) AND webdock_rls.recovery_policy_live(p.id,p.cluster_id)));

-- Old capabilities remain accepted for unrelated domains; recovery requires physical pool binding.
CREATE OR REPLACE FUNCTION webdock_rls.recovery_policy_live(policy text,cluster text) RETURNS boolean
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE policy_row webdock_auth.hosting_recovery_policy%ROWTYPE; result boolean;
BEGIN
 SELECT * INTO policy_row FROM webdock_auth.hosting_recovery_policy p WHERE p.id=policy AND p.cluster_id=cluster FOR SHARE;
 IF NOT FOUND OR (webdock_rls.worker(ARRAY['agent'],cluster) OR webdock_rls.operator()) IS NOT TRUE THEN RETURN false;END IF;
 PERFORM u.id FROM webdock_auth."user" u JOIN webdock_admin.users mapped ON mapped.auth_subject=u.id AND mapped.role='operator'
 WHERE u.id=policy_row.approved_by AND u.role='operator' AND u."twoFactorEnabled" AND u."emailVerified" AND NOT coalesce(u.banned,false) AND NOT u."mustChangePassword" FOR SHARE OF u,mapped;
 IF NOT FOUND THEN RETURN false;END IF;
 IF policy_row.profile='ntfy-sqlite-offline-v1' THEN
  PERFORM app.id FROM webdock_auth.hosting_app app JOIN webdock_auth.hosting_project hp ON hp.project_id=app.project_id JOIN webdock_admin.projects project ON project.id=app.project_id JOIN webdock_admin.customers customer ON customer.id=project.customer_id
  WHERE app.id=policy_row.source_app_id AND hp.cluster_id=cluster AND project.status='active' AND customer.status='active' FOR SHARE OF app,hp,project,customer;
  IF NOT FOUND THEN RETURN false;END IF;
 END IF;
 SELECT (webdock_rls.worker(ARRAY['agent'],cluster) OR webdock_rls.operator()) AND EXISTS(
  SELECT 1 FROM webdock_auth.hosting_recovery_policy p
  JOIN webdock_auth."user" u ON u.id=p.approved_by
  JOIN webdock_auth.hosting_cluster cl ON cl.id=p.cluster_id
  JOIN webdock_auth.hosting_agent a ON a.cluster_id=cl.id
  WHERE p.id=policy AND p.cluster_id=cluster AND p.revoked_at IS NULL
   AND u.role='operator' AND u."twoFactorEnabled" AND u."emailVerified"
   AND NOT coalesce(u.banned,false) AND NOT u."mustChangePassword"
   AND cl.ownership='platform' AND cl.verified AND NOT a.revoked
   AND a.last_seen>clock_timestamp()-interval '90 seconds'
   AND a.observation#>>'{capabilities,nativeRecovery,version}'='2'
   AND p.destination ? 'storageIdentity' AND (a.observation#>'{capabilities,nativeRecovery,profiles}') @> jsonb_build_array(p.destination)
   AND EXISTS(SELECT 1 FROM webdock_auth.plugin_installation i WHERE i.id='hosting' AND i.state='enabled')
   AND ((p.profile='mail-installation-v1'
     AND EXISTS(SELECT 1 FROM webdock_auth.plugin_installation i WHERE i.id='mail' AND i.state IN('enabled','draining') AND i.generation=(p.source->>'mailGeneration')::bigint AND i.revision=(p.source->>'mailRevision')::bigint AND i.version=p.source->>'mailVersion')
     AND EXISTS(SELECT 1 FROM webdock_auth.native_recovery_epoch e WHERE e.singleton AND
       (e.state='active' OR e.state IN('fencing','fenced') AND EXISTS(SELECT 1 FROM webdock_auth.hosting_recovery_job j JOIN webdock_auth.hosting_operation o ON o.id=j.operation_id WHERE j.operation_id=e.operation_id AND j.policy_id=p.id AND o.status='running' AND o.generation=e.generation AND o.lease_until>clock_timestamp()))))
    OR (p.profile='ntfy-sqlite-offline-v1' AND EXISTS(SELECT 1 FROM webdock_auth.native_recovery_epoch app_epoch WHERE app_epoch.singleton AND app_epoch.state='active') AND EXISTS(
     SELECT 1 FROM webdock_auth.hosting_app app
     JOIN webdock_auth.hosting_project hp ON hp.project_id=app.project_id
     JOIN webdock_admin.projects pr ON pr.id=app.project_id
     JOIN webdock_admin.customers c ON c.id=pr.customer_id
     WHERE app.id=p.source_app_id AND hp.cluster_id=cluster AND pr.status='active' AND c.status='active'
      AND app.revision=(p.source->>'revision')::integer AND app.spec=p.source->'spec'
      AND app.observed_uid=p.source->>'runtimeUID' AND app.status='ready'
      AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=pr.id AND d.status<>'deleted'))))) INTO result;
 RETURN coalesce(result,false);
END
$$;
ALTER FUNCTION webdock_rls.recovery_policy_live(text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.recovery_policy_live(text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.recovery_policy_live(text,text) TO webdock_worker_runtime,webdock_auth_runtime;


-- Fresh installation recovery resources and complete native-pool peak; no tenant/project aliases.
CREATE TABLE webdock_auth.hosting_recovery_reservation (
 id varchar PRIMARY KEY,operation_id varchar NOT NULL UNIQUE REFERENCES webdock_auth.hosting_recovery_job(operation_id),
 policy_id varchar NOT NULL REFERENCES webdock_auth.hosting_recovery_policy(id),policy_revision integer NOT NULL,
 cluster_id varchar NOT NULL REFERENCES webdock_auth.hosting_cluster(id),pool_id uuid NOT NULL,pool_identity jsonb NOT NULL,
 target_nonce uuid NOT NULL UNIQUE,target_sha256 text NOT NULL CHECK(target_sha256~'^[a-f0-9]{64}$'),
 source_set_id varchar REFERENCES webdock_auth.hosting_recovery_set(id),peak jsonb NOT NULL,
 peak_bytes bigint NOT NULL CHECK(peak_bytes BETWEEN 67108864 AND 1099511627776),
 demand jsonb NOT NULL,charged_via_hosting boolean NOT NULL,state text NOT NULL CHECK(state IN('reserved','held','released','needs-reconciliation')),
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE webdock_auth.hosting_recovery_target_volume (
 resource_id varchar PRIMARY KEY,reservation_id varchar NOT NULL REFERENCES webdock_auth.hosting_recovery_reservation(id),
 role_name text NOT NULL,role jsonb NOT NULL,original_customer_id varchar,
 volume_bytes bigint NOT NULL CHECK(volume_bytes BETWEEN 67108864 AND 68719476736),image text NOT NULL,
 uid integer NOT NULL CHECK(uid BETWEEN 1 AND 65535),gid integer NOT NULL CHECK(gid BETWEEN 1 AND 65535),
 cpu_millicores integer NOT NULL CHECK(cpu_millicores BETWEEN 100 AND 8000),memory_bytes bigint NOT NULL CHECK(memory_bytes BETWEEN 134217728 AND 8589934592),
 tmp_bytes bigint NOT NULL CHECK(tmp_bytes BETWEEN 1048576 AND 1073741824),UNIQUE(reservation_id,role_name),
 CHECK((role->>'role'='incoming-ce' AND original_customer_id IS NOT NULL AND role->>'originalCustomerID'=original_customer_id AND uid=65532 AND gid=65532)
 OR(role->>'role' IN('core-ledger-postgres','private-mail-postgres') AND original_customer_id IS NULL AND NOT role ? 'originalCustomerID'))
);
CREATE FUNCTION webdock_rls.recovery_reservation_immutable() RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 IF TG_OP='DELETE' OR TG_TABLE_NAME='hosting_recovery_target_volume' THEN RAISE EXCEPTION 'Immutable recovery allocation custody' USING ERRCODE='42501';END IF;
 IF (to_jsonb(NEW)-'state') IS DISTINCT FROM (to_jsonb(OLD)-'state') OR NEW.state<>'held' OR OLD.state<>'reserved' THEN RAISE EXCEPTION 'Immutable recovery peak; guarded cleanup is required' USING ERRCODE='42501';END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_rls.recovery_reservation_immutable() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.recovery_reservation_immutable() FROM PUBLIC;
DO $$ DECLARE t text;BEGIN
 FOREACH t IN ARRAY ARRAY['hosting_recovery_reservation','hosting_recovery_target_volume'] LOOP
  EXECUTE format('ALTER TABLE webdock_auth.%I OWNER TO webdock_platform_owner',t);
  EXECUTE format('ALTER TABLE webdock_auth.%I ENABLE ROW LEVEL SECURITY',t);
  EXECUTE format('ALTER TABLE webdock_auth.%I FORCE ROW LEVEL SECURITY',t);
  EXECUTE format('REVOKE ALL ON webdock_auth.%I FROM PUBLIC,webdock_auth_runtime,webdock_worker_runtime',t);
  EXECUTE format('CREATE POLICY platform_owner ON webdock_auth.%I TO webdock_platform_owner USING(true) WITH CHECK(true)',t);
  EXECUTE format('CREATE TRIGGER recovery_allocation_immutable BEFORE UPDATE OR DELETE ON webdock_auth.%I FOR EACH ROW EXECUTE FUNCTION webdock_rls.recovery_reservation_immutable()',t);
 END LOOP;
END $$;
GRANT SELECT,INSERT ON webdock_auth.hosting_recovery_reservation,webdock_auth.hosting_recovery_target_volume TO webdock_auth_runtime;
GRANT SELECT,UPDATE(state) ON webdock_auth.hosting_recovery_reservation TO webdock_worker_runtime;
GRANT SELECT ON webdock_auth.hosting_recovery_target_volume TO webdock_worker_runtime;
CREATE POLICY recovery_reservation_operator ON webdock_auth.hosting_recovery_reservation TO webdock_auth_runtime USING(webdock_rls.operator()) WITH CHECK(webdock_rls.operator() AND EXISTS(SELECT 1 FROM webdock_auth.hosting_recovery_policy p WHERE p.id=policy_id AND p.revision=policy_revision AND p.revoked_at IS NULL AND p.cluster_id=hosting_recovery_reservation.cluster_id));
CREATE POLICY recovery_volume_operator ON webdock_auth.hosting_recovery_target_volume TO webdock_auth_runtime USING(webdock_rls.operator()) WITH CHECK(webdock_rls.operator() AND EXISTS(SELECT 1 FROM webdock_auth.hosting_recovery_reservation r WHERE r.id=reservation_id));
CREATE POLICY recovery_reservation_worker ON webdock_auth.hosting_recovery_reservation TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['agent'],cluster_id)) WITH CHECK(webdock_rls.worker(ARRAY['agent'],cluster_id) AND EXISTS(SELECT 1 FROM webdock_auth.hosting_recovery_job j JOIN webdock_auth.hosting_operation o ON o.id=j.operation_id JOIN webdock_auth.hosting_recovery_policy p ON p.id=j.policy_id JOIN webdock_auth.hosting_agent a ON a.cluster_id=o.cluster_id WHERE j.operation_id=hosting_recovery_reservation.operation_id AND o.status='running' AND o.lease_until>clock_timestamp() AND j.agent_generation=a.generation AND NOT a.revoked AND p.revision=j.policy_revision AND webdock_rls.recovery_policy_live(p.id,p.cluster_id)));
CREATE POLICY recovery_volume_worker ON webdock_auth.hosting_recovery_target_volume TO webdock_worker_runtime USING(EXISTS(SELECT 1 FROM webdock_auth.hosting_recovery_reservation r WHERE r.id=reservation_id AND webdock_rls.worker(ARRAY['agent'],r.cluster_id)));

-- Ordinary native admission sees retained recovery resources too.
CREATE OR REPLACE FUNCTION webdock_rls.capacity_demand(cluster_id text, demand_kind text, excluded_customer text DEFAULT NULL::text) RETURNS jsonb
    LANGUAGE plpgsql STABLE SECURITY DEFINER
    SET search_path TO 'pg_catalog'
    AS $$
DECLARE result jsonb; reserved_volume numeric; approved numeric; observed numeric; has_pools boolean;
BEGIN
 IF webdock_rls.capacity_cluster_allowed(cluster_id) IS NOT TRUE OR demand_kind IS NULL OR demand_kind NOT IN ('apps','native','all') THEN RAISE EXCEPTION 'Capacity authorization required' USING ERRCODE='42501'; END IF;
 IF excluded_customer IS NOT NULL AND webdock_rls.customer_allowed(excluded_customer) IS NOT TRUE THEN RAISE EXCEPTION 'Capacity exclusion denied' USING ERRCODE='42501'; END IF;
 SELECT COALESCE(sum(p.capacity_bytes),0),count(*)>0 INTO reserved_volume,has_pools FROM webdock_auth.hosting_database_pool p WHERE p.cluster_id=capacity_demand.cluster_id;
 SELECT reserved_volume+COALESCE(sum((b.demand->>'volumeBytes')::numeric),0) INTO reserved_volume FROM webdock_auth.hosting_database_backend b WHERE b.cluster_id=capacity_demand.cluster_id AND b.state<>'purged' AND NOT EXISTS(SELECT 1 FROM webdock_auth.hosting_database_pool p WHERE p.cluster_id=b.cluster_id AND p.pool_id=b.pool_id);
 SELECT (c.capacity->>'volumeBytes')::numeric,(a.observation->'capacity'->>'volumeBytes')::numeric,
 has_pools OR EXISTS(SELECT 1 FROM webdock_auth.hosting_database_backend b WHERE b.cluster_id=c.id AND b.state<>'purged') OR a.observation->'capabilities'->>'databaseVersion'='1'
 INTO approved,observed,has_pools FROM webdock_auth.hosting_cluster c LEFT JOIN webdock_auth.hosting_agent a ON a.cluster_id=c.id WHERE c.id=capacity_demand.cluster_id;
 IF has_pools AND approved IS NOT NULL AND observed IS NOT NULL THEN reserved_volume=greatest(reserved_volume,approved-observed); END IF;
 WITH reservations AS (
  SELECT r.demand,CASE WHEN r.state='held' THEN 'active' ELSE 'reserved' END AS status FROM webdock_auth.hosting_recovery_reservation r WHERE r.cluster_id=capacity_demand.cluster_id AND r.state<>'released' AND NOT r.charged_via_hosting AND demand_kind IN('native','all')
  UNION ALL
  SELECT r.demand,r.status FROM webdock_auth.hosting_reservation r WHERE r.cluster_id=capacity_demand.cluster_id AND r.status<>'released' AND demand_kind IN ('apps','all')
  UNION ALL SELECT r.demand,r.status FROM webdock_mail.cluster_reservation r WHERE r.cluster_id=capacity_demand.cluster_id AND (excluded_customer IS NULL OR r.customer_id<>excluded_customer) AND demand_kind IN ('native','all')
  UNION ALL SELECT b.demand||jsonb_build_object('volumeBytes',0),CASE WHEN b.state IN ('provisioning','updating') THEN 'reserved' ELSE 'active' END FROM webdock_auth.hosting_database_backend b WHERE b.cluster_id=capacity_demand.cluster_id AND b.state<>'purged' AND demand_kind IN ('native','all')
  UNION ALL SELECT jsonb_build_object('volumeBytes',reserved_volume),'active' WHERE demand_kind IN ('native','all')
 ), dimensions AS (SELECT unnest(ARRAY['apps','cpuMillicores','memoryBytes','volumeBytes','ephemeralBytes','replicasPerApp','concurrentDeployments']) AS key),
 amounts AS (SELECT d.key,CASE WHEN d.key='concurrentDeployments' AND r.status='active' THEN 0 ELSE COALESCE((r.demand->>d.key)::numeric,0) END AS value FROM dimensions d LEFT JOIN reservations r ON true),
 totals AS (SELECT key,CASE WHEN key='replicasPerApp' THEN max(value) ELSE sum(value) END AS value,min(value) AS minimum,bool_and(value=trunc(value)) AS integral FROM amounts GROUP BY key)
 SELECT jsonb_object_agg(key,value) INTO result FROM totals WHERE integral AND minimum>=0 AND value BETWEEN 0 AND 9007199254740991 AND value=trunc(value);
 IF (SELECT count(*) FROM jsonb_object_keys(result))<>7 THEN RAISE EXCEPTION 'Invalid cluster reservation'; END IF;
 RETURN result;
END $$;


ALTER FUNCTION webdock_rls.capacity_demand(cluster_id text, demand_kind text, excluded_customer text) OWNER TO webdock_platform_owner;

-- App source custody shares the Mail custody/event ledger, never the Mail epoch transition.
CREATE FUNCTION webdock_rls.recovery_app_effect_allowed(app text,committed_operation text DEFAULT NULL,committed_generation integer DEFAULT NULL,committed_fingerprint text DEFAULT NULL) RETURNS boolean
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE a webdock_auth.hosting_app%ROWTYPE;f webdock_auth.hosting_recovery_source_fence%ROWTYPE;
BEGIN
 SELECT * INTO a FROM webdock_auth.hosting_app WHERE id=app FOR SHARE NOWAIT;
 IF NOT FOUND OR (webdock_rls.project_permission(a.project_id,'org:projects:manage') OR webdock_rls.agent_project(a.project_id) OR webdock_rls.worker(ARRAY['git-worker'])) IS NOT TRUE THEN
  RAISE EXCEPTION 'Current application authority required' USING ERRCODE='42501';END IF;
 SELECT * INTO f FROM webdock_auth.hosting_recovery_source_fence WHERE source_kind='app' AND source_app_id=app AND state<>'released';
 IF NOT FOUND THEN RETURN true;END IF;
 -- An ACK is not new work. Only the exact canonical completed source operation qualifies.
 RETURN coalesce(committed_operation=f.source_operation_id AND committed_generation=f.source_operation_generation
  AND committed_fingerprint=f.source_operation_fingerprint AND EXISTS(SELECT 1 FROM webdock_auth.hosting_operation o
   WHERE o.id=committed_operation AND o.app_id=app AND o.generation=committed_generation AND o.status='succeeded'
    AND o.payload_hash=committed_fingerprint),false);
END $$;
ALTER FUNCTION webdock_rls.recovery_app_effect_allowed(text,text,integer,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.recovery_app_effect_allowed(text,text,integer,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.recovery_app_effect_allowed(text,text,integer,text) TO webdock_auth_runtime,webdock_worker_runtime;

CREATE FUNCTION webdock_rls.recovery_app_source_binding(operation text) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
 SELECT jsonb_build_object('appID',f.source_app_id,'projectID',p.source->>'projectID','customerID',p.source->>'customerID','clusterID',p.cluster_id,
  'policyID',f.policy_id,'policyRevision',f.policy_revision,'agentGeneration',f.agent_generation,'sourceRevision',f.source_app_revision,
  'sourceUID',f.source_observed_uid,'specSHA256',f.source_spec_sha256,'environmentSHA256',f.source_environment_sha256,
  'sourceOperationID',f.source_operation_id,'sourceOperationGeneration',f.source_operation_generation,
  'sourceOperationStatus',f.source_operation_status,'sourceOperationFingerprint',f.source_operation_fingerprint,
  'operationID',f.operation_id,'generation',f.operation_generation,'epoch',f.epoch,'fenceFingerprint',f.plan_sha256)
 FROM webdock_auth.hosting_recovery_source_fence f JOIN webdock_auth.hosting_recovery_policy p ON p.id=f.policy_id
 WHERE f.operation_id=operation AND f.source_kind='app'
$$;
ALTER FUNCTION webdock_rls.recovery_app_source_binding(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.recovery_app_source_binding(text) FROM PUBLIC;

-- Called on a fresh worker transaction, before any agent/policy/op row lock is retained.
-- NOWAIT/advisory try-lock failures abort the whole attempt; no savepoint retry is authority.
CREATE FUNCTION webdock_rls.recovery_app_source_lock(operation text,generation integer,token_hash text,expected_epoch uuid,agent_generation integer) RETURNS boolean
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE j webdock_auth.hosting_recovery_job%ROWTYPE;p webdock_auth.hosting_recovery_policy%ROWTYPE;a webdock_auth.hosting_app%ROWTYPE;o webdock_auth.hosting_operation%ROWTYPE;agent webdock_auth.hosting_agent%ROWTYPE;
BEGIN
 SELECT * INTO j FROM webdock_auth.hosting_recovery_job WHERE operation_id=operation;
 SELECT * INTO p FROM webdock_auth.hosting_recovery_policy WHERE id=j.policy_id;
 IF p.profile IS DISTINCT FROM 'ntfy-sqlite-offline-v1' OR j.spec->>'action' IS DISTINCT FROM 'backup' OR webdock_rls.worker(ARRAY['agent'],p.cluster_id) IS NOT TRUE THEN
  RAISE EXCEPTION 'App recovery scope required' USING ERRCODE='42501';END IF;
 PERFORM epoch FROM webdock_auth.native_recovery_epoch WHERE singleton AND epoch=expected_epoch AND state='active' FOR SHARE NOWAIT;
 IF NOT FOUND THEN RAISE EXCEPTION 'Current active recovery epoch required' USING ERRCODE='42501';END IF;
 IF (pg_try_advisory_xact_lock(hashtextextended('plan:'||(p.source->>'customerID'),0))
  AND pg_try_advisory_xact_lock(hashtextextended('hosting-cluster:'||p.cluster_id,0))) IS NOT TRUE THEN
  RAISE EXCEPTION 'Application recovery lock is busy' USING ERRCODE='55P03';END IF;
 SELECT * INTO a FROM webdock_auth.hosting_app WHERE id=p.source_app_id FOR UPDATE NOWAIT;
 SELECT * INTO o FROM webdock_auth.hosting_operation WHERE id=operation FOR UPDATE NOWAIT;
 PERFORM operation_id FROM webdock_auth.hosting_recovery_job WHERE operation_id=operation FOR UPDATE NOWAIT;
 PERFORM id FROM webdock_auth.hosting_recovery_policy WHERE id=p.id FOR SHARE NOWAIT;
 SELECT * INTO agent FROM webdock_auth.hosting_agent WHERE cluster_id=p.cluster_id FOR SHARE NOWAIT;
 SELECT * INTO j FROM webdock_auth.hosting_recovery_job WHERE operation_id=operation;
 SELECT * INTO p FROM webdock_auth.hosting_recovery_policy WHERE id=j.policy_id;
 -- Prelock every row used by recovery_policy_live without waiting behind an
 -- auth/customer transaction that may itself wait for our plan/cluster lock.
 PERFORM u.id FROM webdock_auth."user" u JOIN webdock_admin.users mapped ON mapped.auth_subject=u.id AND mapped.role='operator'
  WHERE u.id=p.approved_by FOR SHARE OF u,mapped NOWAIT;
 PERFORM app.id FROM webdock_auth.hosting_app app JOIN webdock_auth.hosting_project hp ON hp.project_id=app.project_id
  JOIN webdock_admin.projects project ON project.id=app.project_id JOIN webdock_admin.customers customer ON customer.id=project.customer_id
  WHERE app.id=a.id AND hp.customer_id=p.source->>'customerID' AND project.customer_id=hp.customer_id FOR SHARE OF hp,project,customer NOWAIT;
 IF NOT FOUND THEN RAISE EXCEPTION 'Current application ownership required' USING ERRCODE='42501';END IF;
 IF EXISTS(SELECT 1 FROM webdock_auth.hosting_recovery_source_fence f LEFT JOIN webdock_auth.hosting_operation normal ON normal.id=f.source_operation_id
  WHERE f.operation_id=operation AND (f.source_kind<>'app' OR f.source_app_id IS DISTINCT FROM a.id
   OR f.source_app_revision IS DISTINCT FROM a.revision OR f.source_observed_uid IS DISTINCT FROM a.observed_uid
   OR f.source_spec_sha256 IS DISTINCT FROM encode(sha256(convert_to(a.spec::text,'UTF8')),'hex')
   OR f.source_environment_sha256 IS DISTINCT FROM encode(sha256(convert_to(coalesce(a.environment_encrypted,''),'UTF8')),'hex')
   OR f.source_operation_id IS DISTINCT FROM a.operation_id OR f.source_operation_generation IS DISTINCT FROM normal.generation
   OR f.source_operation_status IS DISTINCT FROM normal.status OR f.source_operation_fingerprint IS DISTINCT FROM normal.payload_hash)) THEN
  RAISE EXCEPTION 'Application source intent changed' USING ERRCODE='42501';END IF;
 IF NOT coalesce(o.status='running' AND o.generation=recovery_app_source_lock.generation AND o.lease_until>clock_timestamp()
  AND j.lease_hash=token_hash AND j.agent_generation=recovery_app_source_lock.agent_generation AND agent.generation=j.agent_generation AND NOT agent.revoked
  AND j.policy_revision=p.revision AND j.spec->>'epoch'=expected_epoch::text AND webdock_rls.recovery_policy_live(p.id,p.cluster_id)
  AND a.project_id=p.source->>'projectID' AND a.revision=(p.source->>'revision')::integer AND a.spec=p.source->'spec'
  AND a.observed_uid=p.source->>'runtimeUID' AND a.status='ready' AND a.observed_revision=a.revision,false) THEN
  RAISE EXCEPTION 'Current exact application recovery lease required' USING ERRCODE='42501';END IF;
 RETURN true;
END $$;
ALTER FUNCTION webdock_rls.recovery_app_source_lock(text,integer,text,uuid,integer) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.recovery_app_source_lock(text,integer,text,uuid,integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.recovery_app_source_lock(text,integer,text,uuid,integer) TO webdock_worker_runtime;

-- Fresh per-effect authority, including periodic Agent guards. Never borrow Mail's epoch UPDATE.
CREATE FUNCTION webdock_rls.recovery_app_source_guard(operation text,generation integer,token_hash text,expected_epoch uuid,agent_generation integer) RETURNS boolean
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE j webdock_auth.hosting_recovery_job%ROWTYPE;f webdock_auth.hosting_recovery_source_fence%ROWTYPE;
BEGIN
 PERFORM webdock_rls.recovery_app_source_lock(operation,generation,token_hash,expected_epoch,agent_generation);
 SELECT * INTO j FROM webdock_auth.hosting_recovery_job WHERE operation_id=operation;
 SELECT * INTO f FROM webdock_auth.hosting_recovery_source_fence WHERE operation_id=operation FOR SHARE NOWAIT;
 IF NOT FOUND THEN RETURN (j.checkpoint IS NULL OR j.checkpoint->>'phase'='preflight') IS TRUE;END IF;
 IF f.source_kind IS DISTINCT FROM 'app' OR f.operation_generation IS DISTINCT FROM generation
  OR f.agent_generation IS DISTINCT FROM agent_generation OR f.epoch IS DISTINCT FROM expected_epoch
  OR f.policy_id IS DISTINCT FROM j.policy_id OR f.policy_revision IS DISTINCT FROM j.policy_revision THEN RETURN false;END IF;
 IF f.state<>'draining' AND j.checkpoint->>'fenceFingerprint' IS DISTINCT FROM f.plan_sha256 THEN RETURN false;END IF;
 IF (CASE f.state
  WHEN 'draining' THEN j.checkpoint->>'phase'='app-admit' AND j.checkpoint->'participants'=f.participants
  WHEN 'stopped' THEN j.checkpoint->>'phase'='app-stopped'
  WHEN 'captured' THEN j.checkpoint->>'phase' IN('app-captured','uploaded-verified')
  WHEN 'resuming' THEN j.checkpoint->>'phase'='app-resume-intent'
  WHEN 'released' THEN j.checkpoint->>'phase'='app-resumed'
  ELSE false END) IS NOT TRUE THEN RETURN false;END IF;
 RETURN true;
END $$;
ALTER FUNCTION webdock_rls.recovery_app_source_guard(text,integer,text,uuid,integer) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.recovery_app_source_guard(text,integer,text,uuid,integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.recovery_app_source_guard(text,integer,text,uuid,integer) TO webdock_worker_runtime;

-- One explicit phase table for app provider effects; caller purpose alone never authorizes.
CREATE FUNCTION webdock_rls.recovery_app_effect_guard(operation text,generation integer,token_hash text,expected_epoch uuid,agent_generation integer,purpose text,expected_fingerprint text) RETURNS boolean
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE j webdock_auth.hosting_recovery_job%ROWTYPE;f webdock_auth.hosting_recovery_source_fence%ROWTYPE;
BEGIN
 IF coalesce(expected_fingerprint,'')!~'^[a-f0-9]{64}$' OR webdock_rls.recovery_app_source_guard(operation,generation,token_hash,expected_epoch,agent_generation) IS NOT TRUE THEN RETURN false;END IF;
 SELECT * INTO j FROM webdock_auth.hosting_recovery_job WHERE operation_id=operation;
 SELECT * INTO f FROM webdock_auth.hosting_recovery_source_fence WHERE operation_id=operation AND source_kind='app' FOR SHARE NOWAIT;
 IF NOT FOUND OR f.plan_sha256 IS DISTINCT FROM expected_fingerprint THEN RETURN false;END IF;
 RETURN EXISTS(SELECT 1 FROM (VALUES
   ('preflight','draining','app-admit'),
   ('stop','draining','app-admit'),
   ('capture','stopped','app-stopped'),
   ('upload','captured','app-captured'),
   ('resume','resuming','app-resume-intent'),
   ('release','released','app-resumed')
  ) AS allowed(effect,source_state,checkpoint_phase)
  WHERE allowed.effect=purpose AND allowed.source_state=f.state AND allowed.checkpoint_phase=j.checkpoint->>'phase')
  AND (purpose<>'release' OR (j.checkpoint->>'providerOutcome'='confirmed-complete' AND j.checkpoint->>'runtimeUID'=f.source_observed_uid)) IS TRUE;
END $$;
ALTER FUNCTION webdock_rls.recovery_app_effect_guard(text,integer,text,uuid,integer,text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.recovery_app_effect_guard(text,integer,text,uuid,integer,text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.recovery_app_effect_guard(text,integer,text,uuid,integer,text,text) TO webdock_worker_runtime;

DROP FUNCTION IF EXISTS webdock_rls.recovery_app_source_admit(text,integer,text,uuid,integer,jsonb);
CREATE FUNCTION webdock_rls.recovery_app_source_admit(operation text,generation integer,token_hash text,expected_epoch uuid,agent_generation integer,participants jsonb,event_id text,audit_id text) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE j webdock_auth.hosting_recovery_job%ROWTYPE;p webdock_auth.hosting_recovery_policy%ROWTYPE;a webdock_auth.hosting_app%ROWTYPE;normal webdock_auth.hosting_operation%ROWTYPE;f webdock_auth.hosting_recovery_source_fence%ROWTYPE;enrolled record;part jsonb;plan_hash text;
BEGIN
 IF webdock_rls.recovery_app_source_guard(operation,generation,token_hash,expected_epoch,agent_generation) IS NOT TRUE THEN
  RAISE EXCEPTION 'Current application source phase required' USING ERRCODE='42501';END IF;
 SELECT * INTO j FROM webdock_auth.hosting_recovery_job WHERE operation_id=operation;
 SELECT * INTO p FROM webdock_auth.hosting_recovery_policy WHERE id=j.policy_id;
 SELECT * INTO a FROM webdock_auth.hosting_app WHERE id=p.source_app_id;
 SELECT * INTO normal FROM webdock_auth.hosting_operation WHERE id=a.operation_id FOR SHARE NOWAIT;
 IF normal.status IS DISTINCT FROM 'succeeded' OR normal.app_id IS DISTINCT FROM a.id OR EXISTS(SELECT 1 FROM webdock_auth.hosting_operation o
   WHERE o.app_id=a.id AND o.id<>operation AND o.status IN('queued','running','needs-reconciliation')) THEN
  RAISE EXCEPTION 'Previous application effects remain undrained' USING ERRCODE='55000';END IF;
 IF jsonb_typeof(participants) IS DISTINCT FROM 'array' OR jsonb_array_length(participants) NOT BETWEEN 1 AND 200
  OR octet_length(participants::text)>65536 OR (SELECT count(DISTINCT v->>'identity') FROM jsonb_array_elements(participants) v)<>jsonb_array_length(participants)
  OR EXISTS(SELECT 1 FROM jsonb_array_elements(participants) v WHERE jsonb_typeof(v)<>'object' OR coalesce(v->>'kind','') NOT IN('guarded-agent','legacy-worker')
    OR coalesce(v->>'ownerSha256','')!~'^[a-f0-9]{64}$') THEN
  RAISE EXCEPTION 'Exact application executor inventory required' USING ERRCODE='42501';END IF;
 FOR enrolled IN SELECT * FROM webdock_auth.agent_transport_enrollment e WHERE e.cluster_id=p.cluster_id AND NOT e.revoked LOOP
  SELECT value INTO part FROM jsonb_array_elements(participants) WHERE value->>'identity'='agent:'||enrolled.installation||':'||enrolled.agent_id
   AND value->>'uid'=enrolled.credential_hash AND (value->>'agentGeneration')::integer=enrolled.generation
   AND value->>'ownerSha256'=encode(sha256(convert_to('agent:'||enrolled.installation||':'||enrolled.agent_id||':'||enrolled.credential_hash,'UTF8')),'hex');
  IF NOT FOUND OR (part->>'kind'='guarded-agent' AND (j.agent_identity->>'identity' IS DISTINCT FROM part->>'identity'
    OR j.agent_identity->>'uid' IS DISTINCT FROM part->>'uid' OR part->>'effectGuardVersion' IS DISTINCT FROM 'app-1')) THEN
   RAISE EXCEPTION 'Known application executor is not accounted for' USING ERRCODE='55000';END IF;
 END LOOP;
 IF j.agent_identity IS NULL OR NOT EXISTS(SELECT 1 FROM jsonb_array_elements(participants) v WHERE v->>'identity'=j.agent_identity->>'identity'
  AND v->>'uid'=j.agent_identity->>'uid' AND v->>'kind'='guarded-agent' AND v->>'effectGuardVersion'='app-1')
  OR EXISTS(SELECT 1 FROM jsonb_array_elements(participants) v WHERE NOT EXISTS(SELECT 1 FROM webdock_auth.agent_transport_enrollment e
   WHERE e.cluster_id=p.cluster_id AND NOT e.revoked AND v->>'identity'='agent:'||e.installation||':'||e.agent_id AND v->>'uid'=e.credential_hash))
  OR NOT EXISTS(SELECT 1 FROM webdock_auth.hosting_agent agent WHERE agent.cluster_id=p.cluster_id AND agent.observation#>'{capabilities,nativeRecovery,appEffectFenceVersion}'='1'::jsonb) THEN
  RAISE EXCEPTION 'Current persistent application executor required' USING ERRCODE='42501';END IF;
 plan_hash:=encode(sha256(convert_to(jsonb_build_object('participants',participants,'policyID',p.id,'policyRevision',p.revision,
  'appID',a.id,'revision',a.revision,'uid',a.observed_uid,'spec',a.spec,'environmentSHA256',encode(sha256(convert_to(coalesce(a.environment_encrypted,''),'UTF8')),'hex'),
  'normalOperation',normal.id,'normalGeneration',normal.generation,'normalFingerprint',normal.payload_hash)::text,'UTF8')),'hex');
 SELECT * INTO f FROM webdock_auth.hosting_recovery_source_fence WHERE operation_id=operation FOR UPDATE NOWAIT;
 IF FOUND THEN
  IF f.source_kind<>'app' OR f.state='released' OR f.plan_sha256<>plan_hash OR f.operation_generation<>generation OR f.agent_generation<>agent_generation OR f.epoch<>expected_epoch THEN
   RAISE EXCEPTION 'Application fence changed' USING ERRCODE='42501';END IF;
 ELSE
 -- Core-native history IDs, never checkpoint input.
 IF ((CASE WHEN event_id ~ '^[1-9][0-9]{0,18}$' THEN event_id::numeric<=9223372036854775807 ELSE false END) AND (CASE WHEN audit_id ~ '^[1-9][0-9]{0,18}$' THEN audit_id::numeric<=9223372036854775807 ELSE false END) AND event_id<>audit_id) IS NOT TRUE THEN RAISE EXCEPTION 'Native recovery record IDs required' USING ERRCODE='22023';END IF;
 -- End native history IDs.
  INSERT INTO webdock_auth.hosting_recovery_source_fence(operation_id,policy_id,policy_revision,operation_generation,agent_generation,epoch,source_kind,
   source_app_id,source_app_revision,source_observed_uid,source_spec_sha256,source_environment_sha256,source_operation_id,source_operation_generation,source_operation_status,source_operation_fingerprint,plan_sha256,participants,state)
  VALUES(operation,p.id,p.revision,generation,agent_generation,expected_epoch,'app',a.id,a.revision,a.observed_uid,encode(sha256(convert_to(a.spec::text,'UTF8')),'hex'),
   encode(sha256(convert_to(coalesce(a.environment_encrypted,''),'UTF8')),'hex'),normal.id,normal.generation,normal.status,normal.payload_hash,plan_hash,participants,'draining');
  INSERT INTO webdock_auth.hosting_recovery_source_event(id,operation_id,event,proof) VALUES(event_id,operation,'app-admitted',jsonb_build_object('fenceFingerprint',plan_hash));
  INSERT INTO webdock_auth.hosting_audit(id,subject,action,target_id,outcome) VALUES(audit_id,p.approved_by,'recovery.app-source',a.id,'draining');
 END IF;
 RETURN webdock_rls.recovery_app_source_binding(operation);
END $$;
ALTER FUNCTION webdock_rls.recovery_app_source_admit(text,integer,text,uuid,integer,jsonb,text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.recovery_app_source_admit(text,integer,text,uuid,integer,jsonb,text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.recovery_app_source_admit(text,integer,text,uuid,integer,jsonb,text,text) TO webdock_worker_runtime;

DROP FUNCTION IF EXISTS webdock_rls.recovery_app_source_drained(text,integer,text,uuid,integer,jsonb);
CREATE FUNCTION webdock_rls.recovery_app_source_drained(operation text,generation integer,token_hash text,expected_epoch uuid,agent_generation integer,proof jsonb,event_id text) RETURNS boolean
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE f webdock_auth.hosting_recovery_source_fence%ROWTYPE;part jsonb;outcome jsonb;next_state text;
BEGIN
 IF webdock_rls.recovery_app_source_guard(operation,generation,token_hash,expected_epoch,agent_generation) IS NOT TRUE THEN
  RAISE EXCEPTION 'Current application source phase required' USING ERRCODE='42501';END IF;
 SELECT * INTO f FROM webdock_auth.hosting_recovery_source_fence WHERE operation_id=operation AND source_kind='app' FOR UPDATE NOWAIT;
 next_state:=CASE proof->>'phase' WHEN 'app-stopped' THEN 'stopped' WHEN 'app-captured' THEN 'captured' END;
 IF NOT FOUND OR next_state IS NULL OR proof->>'fenceFingerprint' IS DISTINCT FROM f.plan_sha256
  OR proof->>'runtimeUID' IS DISTINCT FROM f.source_observed_uid OR proof->'writersAbsent' IS DISTINCT FROM 'true'::jsonb
  OR proof->>'sourceUUID' IS NULL OR proof->>'sourceUUID'!~'^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$'
  OR jsonb_typeof(proof->'participants') IS DISTINCT FROM 'array' OR jsonb_array_length(proof->'participants')<>jsonb_array_length(f.participants) THEN RETURN false;END IF;
 IF f.state=next_state THEN RETURN EXISTS(SELECT 1 FROM webdock_auth.hosting_recovery_source_event e WHERE e.operation_id=operation AND e.event=proof->>'phase' AND e.proof=proof);END IF;
 IF (f.state='draining' AND next_state='stopped' OR f.state='stopped' AND next_state='captured') IS NOT TRUE THEN RETURN false;END IF;
 FOR part IN SELECT value FROM jsonb_array_elements(f.participants) LOOP
  SELECT value INTO outcome FROM jsonb_array_elements(proof->'participants') WHERE value->>'identity'=part->>'identity';
  IF NOT FOUND OR outcome->>'uid' IS DISTINCT FROM part->>'uid' OR outcome->>'ownerSha256' IS DISTINCT FROM part->>'ownerSha256'
   OR outcome->'uncertain' IS DISTINCT FROM 'false'::jsonb
   OR (part->>'kind'='guarded-agent' AND (outcome->>'state' IS DISTINCT FROM 'guarded-drained' OR outcome->>'effectGuardVersion' IS DISTINCT FROM 'app-1'))
   OR (part->>'kind'='legacy-worker' AND (coalesce(outcome->>'state','') NOT IN('stopped','completed') OR coalesce(outcome->>'providerOutcome','') NOT IN('not-started','confirmed-complete'))) THEN RETURN false;END IF;
 END LOOP;
 IF next_state='captured' AND coalesce(proof->>'manifestSHA256','')!~'^[a-f0-9]{64}$' THEN RETURN false;END IF;
 -- Core-native history IDs, never checkpoint input.
 IF ((CASE WHEN event_id ~ '^[1-9][0-9]{0,18}$' THEN event_id::numeric<=9223372036854775807 ELSE false END)) IS NOT TRUE THEN RAISE EXCEPTION 'Native recovery record IDs required' USING ERRCODE='22023';END IF;
 -- End native history IDs.
 UPDATE webdock_auth.hosting_recovery_source_fence SET state=next_state WHERE operation_id=operation;
 INSERT INTO webdock_auth.hosting_recovery_source_event(id,operation_id,event,proof) VALUES(event_id,operation,proof->>'phase',proof);
 RETURN true;
END $$;
ALTER FUNCTION webdock_rls.recovery_app_source_drained(text,integer,text,uuid,integer,jsonb,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.recovery_app_source_drained(text,integer,text,uuid,integer,jsonb,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.recovery_app_source_drained(text,integer,text,uuid,integer,jsonb,text) TO webdock_worker_runtime;

DROP FUNCTION IF EXISTS webdock_rls.recovery_app_source_resume(text,integer,text,uuid,integer,jsonb);
CREATE FUNCTION webdock_rls.recovery_app_source_resume(operation text,generation integer,token_hash text,expected_epoch uuid,agent_generation integer,proof jsonb,event_id text,audit_id text) RETURNS boolean
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE f webdock_auth.hosting_recovery_source_fence%ROWTYPE;next_state text;
BEGIN
 IF webdock_rls.recovery_app_source_guard(operation,generation,token_hash,expected_epoch,agent_generation) IS NOT TRUE THEN
  RAISE EXCEPTION 'Current application source phase required' USING ERRCODE='42501';END IF;
 SELECT * INTO f FROM webdock_auth.hosting_recovery_source_fence WHERE operation_id=operation AND source_kind='app' FOR UPDATE NOWAIT;
 next_state:=CASE proof->>'phase' WHEN 'app-resume-intent' THEN 'resuming' WHEN 'app-resumed' THEN 'released' END;
 IF NOT FOUND OR next_state IS NULL OR proof->>'fenceFingerprint' IS DISTINCT FROM f.plan_sha256 OR proof->>'runtimeUID' IS DISTINCT FROM f.source_observed_uid THEN RETURN false;END IF;
 IF f.state=next_state THEN RETURN EXISTS(SELECT 1 FROM webdock_auth.hosting_recovery_source_event e WHERE e.operation_id=operation AND e.event=proof->>'phase' AND e.proof=proof);END IF;
 IF (f.state IN('stopped','captured') AND next_state='resuming' OR f.state='resuming' AND next_state='released' AND proof->>'providerOutcome'='confirmed-complete') IS NOT TRUE THEN RETURN false;END IF;
 -- Core-native history IDs, never checkpoint input.
 IF ((CASE WHEN event_id ~ '^[1-9][0-9]{0,18}$' THEN event_id::numeric<=9223372036854775807 ELSE false END) AND (CASE WHEN audit_id ~ '^[1-9][0-9]{0,18}$' THEN audit_id::numeric<=9223372036854775807 ELSE false END) AND event_id<>audit_id) IS NOT TRUE THEN RAISE EXCEPTION 'Native recovery record IDs required' USING ERRCODE='22023';END IF;
 -- End native history IDs.
 UPDATE webdock_auth.hosting_recovery_source_fence SET state=next_state WHERE operation_id=operation;
 INSERT INTO webdock_auth.hosting_recovery_source_event(id,operation_id,event,proof) VALUES(event_id,operation,proof->>'phase',proof);
 INSERT INTO webdock_auth.hosting_audit(id,subject,action,target_id,outcome) SELECT audit_id,p.approved_by,'recovery.app-compensation',f.source_app_id,next_state FROM webdock_auth.hosting_recovery_policy p WHERE p.id=f.policy_id;
 RETURN true;
END $$;
ALTER FUNCTION webdock_rls.recovery_app_source_resume(text,integer,text,uuid,integer,jsonb,text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.recovery_app_source_resume(text,integer,text,uuid,integer,jsonb,text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.recovery_app_source_resume(text,integer,text,uuid,integer,jsonb,text,text) TO webdock_worker_runtime;
CREATE FUNCTION webdock_rls.recovery_app_source_view(operation text,generation integer,token_hash text,expected_epoch uuid,agent_generation integer) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE j webdock_auth.hosting_recovery_job%ROWTYPE;p webdock_auth.hosting_recovery_policy%ROWTYPE;f webdock_auth.hosting_recovery_source_fence%ROWTYPE;
BEGIN
 IF webdock_rls.recovery_app_source_guard(operation,generation,token_hash,expected_epoch,agent_generation) IS NOT TRUE THEN
  RAISE EXCEPTION 'Current application source phase required' USING ERRCODE='42501';END IF;
 SELECT * INTO j FROM webdock_auth.hosting_recovery_job WHERE operation_id=operation;
 SELECT * INTO p FROM webdock_auth.hosting_recovery_policy WHERE id=j.policy_id;
 SELECT * INTO f FROM webdock_auth.hosting_recovery_source_fence WHERE operation_id=operation AND source_kind='app';
 RETURN jsonb_build_object('version',1,'sourceKind','app','binding',webdock_rls.recovery_app_source_binding(operation),'state',f.state,'participants',f.participants,'capturedManifestSHA256',(SELECT e.proof->>'manifestSHA256' FROM webdock_auth.hosting_recovery_source_event e WHERE e.operation_id=operation AND e.event='app-captured' ORDER BY e.created_at LIMIT 1),'agentIdentity',j.agent_identity,
  'enrolledExecutors',coalesce((SELECT jsonb_agg(jsonb_build_object('identity','agent:'||e.installation||':'||e.agent_id,'uid',e.credential_hash,'agentGeneration',e.generation,
   'ownerSha256',encode(sha256(convert_to('agent:'||e.installation||':'||e.agent_id||':'||e.credential_hash,'UTF8')),'hex')) ORDER BY e.agent_id)
   FROM webdock_auth.agent_transport_enrollment e WHERE e.cluster_id=p.cluster_id AND NOT e.revoked),'[]'::jsonb));
END $$;
ALTER FUNCTION webdock_rls.recovery_app_source_view(text,integer,text,uuid,integer) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.recovery_app_source_view(text,integer,text,uuid,integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.recovery_app_source_view(text,integer,text,uuid,integer) TO webdock_worker_runtime;

-- Core binds an already authenticated transport to one current job; workers never read transport credentials.
CREATE FUNCTION webdock_rls.recovery_bind_transport(requested_operation text,requested_generation integer,requested_token_hash text,expected_epoch uuid,requested_agent_generation integer,requested_installation text,requested_agent text) RETURNS boolean
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE j webdock_auth.hosting_recovery_job%ROWTYPE;p webdock_auth.hosting_recovery_policy%ROWTYPE;o webdock_auth.hosting_operation%ROWTYPE;
 a webdock_auth.hosting_agent%ROWTYPE;cluster webdock_auth.hosting_cluster%ROWTYPE;e webdock_auth.agent_transport_enrollment%ROWTYPE;
 epoch_row webdock_auth.native_recovery_epoch%ROWTYPE;i webdock_auth.plugin_installation%ROWTYPE;bound_identity jsonb;initial_binding boolean;epoch_allowed boolean;
BEGIN
 SELECT * INTO j FROM webdock_auth.hosting_recovery_job WHERE operation_id=requested_operation;
 IF NOT FOUND THEN RETURN false;END IF;
 SELECT * INTO p FROM webdock_auth.hosting_recovery_policy WHERE id=j.policy_id;
 IF NOT FOUND OR webdock_rls.worker(ARRAY['agent'],p.cluster_id) IS NOT TRUE
  OR (p.profile IN('mail-installation-v1','ntfy-sqlite-offline-v1') AND j.spec->>'action' IN('backup','restore')) IS NOT TRUE THEN RETURN false;END IF;
 IF p.profile='ntfy-sqlite-offline-v1' AND j.spec->>'action'='backup' THEN
  -- This owns epoch SHARE NOWAIT before agent/op/transport locks, also on direct calls.
  PERFORM webdock_rls.recovery_app_source_lock(requested_operation,requested_generation,requested_token_hash,expected_epoch,requested_agent_generation);
 ELSE
  -- Generic/Mail ordering retains the agent first; Mail's epoch UPDATE precedes installation custody.
  SELECT * INTO a FROM webdock_auth.hosting_agent WHERE cluster_id=p.cluster_id FOR SHARE;
 END IF;
 SELECT * INTO j FROM webdock_auth.hosting_recovery_job WHERE operation_id=requested_operation FOR UPDATE;
 SELECT * INTO p FROM webdock_auth.hosting_recovery_policy WHERE id=j.policy_id FOR SHARE;
 SELECT * INTO o FROM webdock_auth.hosting_operation WHERE id=requested_operation FOR SHARE;
 SELECT * INTO a FROM webdock_auth.hosting_agent WHERE cluster_id=p.cluster_id FOR SHARE;
 SELECT * INTO cluster FROM webdock_auth.hosting_cluster WHERE id=p.cluster_id FOR SHARE;
 IF p.profile='mail-installation-v1' THEN
  SELECT * INTO epoch_row FROM webdock_auth.native_recovery_epoch WHERE singleton FOR UPDATE;
  SELECT * INTO i FROM webdock_auth.plugin_installation WHERE id='mail' FOR SHARE;
 ELSE
  SELECT * INTO epoch_row FROM webdock_auth.native_recovery_epoch WHERE singleton FOR SHARE;
 END IF;
 IF epoch_row.state IS NULL OR epoch_row.state='quarantined' THEN RETURN false;END IF;
 initial_binding:=((j.checkpoint IS NULL OR (jsonb_typeof(j.checkpoint)='object' AND j.checkpoint->>'phase'='preflight')) IS TRUE
  AND NOT EXISTS(SELECT 1 FROM webdock_auth.hosting_recovery_source_fence f WHERE f.operation_id=requested_operation)
  AND epoch_row.state='active' AND (CASE WHEN p.profile='mail-installation-v1' AND j.spec->>'action'='backup'
   THEN epoch_row.epoch<>expected_epoch ELSE epoch_row.epoch=expected_epoch END)) IS TRUE;
 IF p.profile='mail-installation-v1' AND j.spec->>'action'='backup' THEN
  -- The stored job nonce is reserved before fencing; do not confuse it with the live epoch.
  epoch_allowed:=(CASE epoch_row.state
   WHEN 'active' THEN
    (initial_binding AND epoch_row.epoch<>expected_epoch)
    OR (j.agent_identity IS NOT NULL AND epoch_row.epoch=expected_epoch
      AND epoch_row.operation_id=requested_operation AND epoch_row.generation=requested_generation
      AND epoch_row.intent_revision=p.revision AND epoch_row.source_resume_allowed=false
      AND j.checkpoint->>'phase'='same-fence-compensation'
      AND EXISTS(SELECT 1 FROM webdock_auth.hosting_recovery_source_fence f WHERE f.operation_id=requested_operation AND f.source_kind='mail'
        AND f.state='resumed' AND f.epoch=expected_epoch AND f.operation_generation=requested_generation
        AND f.agent_generation=requested_agent_generation AND f.policy_revision=p.revision))
   WHEN 'fencing' THEN epoch_row.epoch=expected_epoch AND epoch_row.operation_id=requested_operation AND epoch_row.generation=requested_generation AND epoch_row.intent_revision=p.revision
   WHEN 'fenced' THEN epoch_row.epoch=expected_epoch AND epoch_row.operation_id=requested_operation AND epoch_row.generation=requested_generation AND epoch_row.intent_revision=p.revision
   ELSE false END) IS TRUE;
 ELSE
  epoch_allowed:=(epoch_row.epoch=expected_epoch AND epoch_row.state='active') IS TRUE;
 END IF;
 IF (webdock_rls.worker(ARRAY['agent'],p.cluster_id) AND o.cluster_id=p.cluster_id
  AND o.status='running' AND o.generation=requested_generation AND o.lease_until>clock_timestamp()
  AND o.action='recovery.'||(j.spec->>'action') AND o.desired=j.spec AND o.payload_hash=j.request_sha256
  AND j.lease_hash=requested_token_hash AND j.agent_generation=requested_agent_generation
  AND a.generation=requested_agent_generation AND NOT a.revoked AND p.revision=j.policy_revision
  AND j.spec->>'operationID'=requested_operation AND j.spec->>'policyID'=p.id
  AND j.spec->>'clusterID'=p.cluster_id AND j.spec->>'epoch'=expected_epoch::text
  AND j.spec->>'profile'=p.profile AND j.spec->>'policyRevision'=p.revision::text
  AND epoch_allowed
  AND cluster.runtime_installation=requested_installation AND cluster.ownership='platform' AND cluster.verified
  AND webdock_rls.recovery_policy_live(p.id,p.cluster_id)
  AND (p.profile<>'mail-installation-v1' OR i.state IN('enabled','draining')
    AND i.generation=(p.source->>'mailGeneration')::bigint AND i.revision=(p.source->>'mailRevision')::bigint AND i.version=p.source->>'mailVersion')) IS NOT TRUE THEN RETURN false;END IF;
 SELECT * INTO e FROM webdock_auth.agent_transport_enrollment transport
  WHERE transport.installation=requested_installation AND transport.agent_id=requested_agent
   AND transport.cluster_id=p.cluster_id AND transport.generation=requested_agent_generation AND NOT transport.revoked FOR SHARE;
 IF NOT FOUND OR (e.credential_hash=a.credential_hash) IS NOT TRUE THEN RETURN false;END IF;
 bound_identity:=jsonb_build_object('identity','agent:'||e.installation||':'||e.agent_id,'uid',e.credential_hash,'generation',e.generation);
 IF j.agent_identity IS NOT NULL THEN RETURN j.agent_identity=bound_identity;END IF;
 IF initial_binding IS NOT TRUE THEN RETURN false;END IF;
 UPDATE webdock_auth.hosting_recovery_job SET agent_identity=bound_identity
  WHERE operation_id=requested_operation AND agent_identity IS NULL;
 RETURN FOUND;
END $$;
ALTER FUNCTION webdock_rls.recovery_bind_transport(text,integer,text,uuid,integer,text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.recovery_bind_transport(text,integer,text,uuid,integer,text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.recovery_bind_transport(text,integer,text,uuid,integer,text,text) TO webdock_worker_runtime;
