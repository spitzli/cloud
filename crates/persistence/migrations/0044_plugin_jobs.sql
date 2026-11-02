-- Immediate native jobs retain the original real actor and a private per-attempt claim.
ALTER TABLE webdock_auth.plugin_job_execution
 ADD COLUMN provenance jsonb,
 ADD COLUMN request_hash text CHECK (request_hash ~ '^[a-f0-9]{64}$'),
 ADD COLUMN max_attempts integer NOT NULL DEFAULT 3 CHECK (max_attempts BETWEEN 1 AND 5),
 ADD COLUMN result jsonb CHECK (octet_length(result::text)<=32768*128),
 ADD COLUMN condition text CHECK (condition IN ('cancelled','authority_revoked','attempts_exhausted','unavailable','deadline_exceeded','invalid_result','attempt_failed','legacy_authority')),
 ADD COLUMN invocation_id uuid REFERENCES webdock_auth.plugin_invocation(id),
 ADD CONSTRAINT plugin_job_provenance_bound CHECK (octet_length(provenance::text)<=65536*128+32768 AND octet_length((provenance-'session_context')::text)<=16384);
-- Compact wire limits remain 32KiB payload/result and 64KiB configuration in Rust.
-- A finite f64 expands to at most 343 characters in numeric text; a scientific token
-- has at least 3 bytes. Factor 128 bounds that expansion and jsonb separator whitespace.
ALTER TABLE webdock_auth.plugin_job_execution DROP CONSTRAINT plugin_job_execution_payload_check;
ALTER TABLE webdock_auth.plugin_job_execution ADD CHECK (octet_length(payload::text)<=32768*128);
ALTER TABLE webdock_auth.plugin_configuration DROP CONSTRAINT plugin_configuration_configuration_check;
ALTER TABLE webdock_auth.plugin_configuration ADD CONSTRAINT plugin_configuration_configuration_check
 CHECK (jsonb_typeof(configuration)='object' AND octet_length(configuration::text)<=65536*128);
-- Retain the original 16KiB ceiling for authority and other context. Only the bounded
-- configuration snapshot receives the expanded allowance; source row overhead stays bounded too.
ALTER TABLE webdock_auth.plugin_invocation DROP CONSTRAINT plugin_invocation_session_context_check;
ALTER TABLE webdock_auth.plugin_invocation ADD CONSTRAINT plugin_invocation_session_context_check
 CHECK (octet_length(session_context::text)<=65536*128+16384 AND octet_length((session_context #- '{nativeAdmission,configuration}')::text)<=16384);
UPDATE webdock_auth.plugin_invocation SET revoked_at=coalesce(revoked_at,now()) WHERE kind='job';
UPDATE webdock_auth.plugin_job_execution SET status='cancelled',cancel_requested=true,claim_token=NULL,lease_until=NULL,condition='legacy_authority' WHERE status IN ('pending','running');
ALTER TABLE webdock_auth.plugin_invocation DROP CONSTRAINT plugin_invocation_check1;
ALTER TABLE webdock_auth.plugin_invocation ADD CONSTRAINT plugin_invocation_native_actor CHECK (
 (kind='action' AND actor_id IS NOT NULL AND session_id IS NOT NULL AND job_execution_id IS NULL AND job_claim_token IS NULL)
 OR (kind='job' AND job_execution_id IS NOT NULL AND job_claim_token IS NOT NULL
 AND ((actor_id IS NOT NULL AND session_id IS NOT NULL) OR revoked_at IS NOT NULL)));

CREATE FUNCTION webdock_rls.plugin_job_names(manifest jsonb, approved jsonb) RETURNS jsonb
LANGUAGE sql IMMUTABLE SET search_path=pg_catalog AS $$
 SELECT coalesce(jsonb_agg(j->>'name'),'[]'::jsonb) FROM jsonb_array_elements(manifest->'jobs') j
 WHERE NOT j ? 'intervalSeconds' AND NOT EXISTS(SELECT 1 FROM jsonb_array_elements_text(j->'capabilities') c
 WHERE c NOT IN ('plugin.data.read','plugin.data.write','plugin.http.read','plugin.http.request') OR NOT approved ? c)
$$;
ALTER FUNCTION webdock_rls.plugin_job_names(jsonb,jsonb) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.plugin_job_names(jsonb,jsonb) FROM PUBLIC;
-- Adding the native job grant is a visible revision change; revoked grants stay revoked.
UPDATE webdock_auth.plugin_installation p SET policy_revision=policy_revision+1
 FROM webdock_auth.plugin_release l WHERE l.plugin_id=p.id AND l.version=p.version AND jsonb_array_length(l.manifest_bytes::jsonb->'jobs')>0;
UPDATE webdock_auth.plugin_grant g SET jobs=webdock_rls.plugin_job_names(l.manifest_bytes::jsonb,c.approved_capabilities),revision=p.policy_revision
 FROM webdock_auth.plugin_installation p JOIN webdock_auth.plugin_configuration c ON c.plugin_id=p.id
 JOIN webdock_auth.plugin_release l ON l.plugin_id=p.id AND l.version=p.version
 WHERE g.plugin_id=p.id AND g.revoked_at IS NULL;
CREATE OR REPLACE FUNCTION webdock_rls.ensure_plugin_grant(plugin text, project text) RETURNS void
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$ BEGIN
 IF NOT webdock_rls.worker(ARRAY['plugin-controller']::text[]) THEN RAISE EXCEPTION 'plugin controller required' USING ERRCODE='42501'; END IF;
 INSERT INTO webdock_auth.plugin_grant(plugin_id,customer_id,project_id,capabilities,revision,jobs)
 SELECT p.id,project_row.customer_id,project_row.id,c.approved_capabilities,p.policy_revision,
 webdock_rls.plugin_job_names(l.manifest_bytes::jsonb,c.approved_capabilities)
 FROM webdock_auth.plugin_installation p JOIN webdock_auth.plugin_configuration c ON c.plugin_id=p.id
 JOIN webdock_auth.plugin_release l ON l.plugin_id=p.id AND l.version=p.version
 JOIN webdock_admin.projects project_row ON project_row.id=project WHERE p.id=plugin AND p.kind='external'
 ON CONFLICT (plugin_id,customer_id,project_id) DO UPDATE SET capabilities=EXCLUDED.capabilities,revision=EXCLUDED.revision,jobs=EXCLUDED.jobs
 WHERE webdock_auth.plugin_grant.revoked_at IS NULL AND webdock_auth.plugin_grant.revision<EXCLUDED.revision;
END $$;

-- Promotion can change job declarations without changing the approved capability set.
CREATE FUNCTION webdock_auth.refresh_native_job_grants() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE names jsonb; next_revision bigint;
BEGIN
 SELECT webdock_rls.plugin_job_names(l.manifest_bytes::jsonb,c.approved_capabilities) INTO names
 FROM webdock_auth.plugin_release l JOIN webdock_auth.plugin_configuration c ON c.plugin_id=l.plugin_id
 WHERE l.plugin_id=NEW.id AND l.version=NEW.version;
 IF names IS NULL OR NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_grant g WHERE g.plugin_id=NEW.id AND g.revoked_at IS NULL AND g.jobs IS DISTINCT FROM names) THEN RETURN NEW; END IF;
 UPDATE webdock_auth.plugin_installation SET policy_revision=policy_revision+1 WHERE id=NEW.id RETURNING policy_revision INTO next_revision;
 UPDATE webdock_auth.plugin_grant SET jobs=names,revision=next_revision WHERE plugin_id=NEW.id AND revoked_at IS NULL;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_auth.refresh_native_job_grants() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.refresh_native_job_grants() FROM PUBLIC;
CREATE TRIGGER refresh_native_job_grants AFTER UPDATE OF current_generation ON webdock_auth.plugin_installation
 FOR EACH ROW WHEN (OLD.current_generation IS DISTINCT FROM NEW.current_generation) EXECUTE FUNCTION webdock_auth.refresh_native_job_grants();

CREATE OR REPLACE FUNCTION webdock_rls.bump_plugin_policy(plugin text) RETURNS bigint
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE new_revision bigint;
BEGIN
 IF NOT webdock_rls.operator() THEN RAISE EXCEPTION 'operator required' USING ERRCODE='42501'; END IF;
 UPDATE webdock_auth.plugin_installation SET policy_revision=policy_revision+1 WHERE id=plugin AND kind='external' RETURNING policy_revision INTO new_revision;
 IF new_revision IS NULL THEN RAISE EXCEPTION 'unknown plugin' USING ERRCODE='P0002'; END IF;
 UPDATE webdock_auth.plugin_grant g SET revision=new_revision,capabilities=c.approved_capabilities,
 jobs=webdock_rls.plugin_job_names(l.manifest_bytes::jsonb,c.approved_capabilities)
 FROM webdock_auth.plugin_configuration c JOIN webdock_auth.plugin_installation p ON p.id=c.plugin_id
 JOIN webdock_auth.plugin_release l ON l.plugin_id=p.id AND l.version=p.version WHERE g.plugin_id=plugin AND c.plugin_id=plugin;
 RETURN new_revision;
END $$;

CREATE FUNCTION webdock_auth.plugin_job_immutable() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog AS $$ BEGIN
 IF (to_jsonb(NEW)-ARRAY['status','claim_token','lease_until','attempts','cancel_requested','available_at','result','condition','invocation_id'])
 IS DISTINCT FROM (to_jsonb(OLD)-ARRAY['status','claim_token','lease_until','attempts','cancel_requested','available_at','result','condition','invocation_id'])
 THEN RAISE EXCEPTION 'job admission is immutable' USING ERRCODE='42501'; END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_auth.plugin_job_immutable() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.plugin_job_immutable() FROM PUBLIC;
CREATE TRIGGER plugin_job_immutable BEFORE UPDATE ON webdock_auth.plugin_job_execution FOR EACH ROW EXECUTE FUNCTION webdock_auth.plugin_job_immutable();

-- Fixed controller purpose; derive every row to lock from one persisted job, never caller scope.
CREATE FUNCTION webdock_rls.lock_plugin_job_metadata(job_id text) RETURNS boolean
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE j webdock_auth.plugin_job_execution%ROWTYPE;
BEGIN
 IF NOT webdock_rls.worker(ARRAY['plugin-controller']::text[]) THEN RETURN false; END IF;
 SELECT * INTO j FROM webdock_auth.plugin_job_execution WHERE id=job_id;
 IF NOT FOUND THEN RETURN false; END IF;
 PERFORM id FROM webdock_admin.customers WHERE id=j.customer_id FOR SHARE;
 PERFORM id FROM webdock_admin.projects WHERE id=j.project_id AND customer_id=j.customer_id FOR SHARE;
 PERFORM id FROM webdock_auth.plugin_installation WHERE id=j.plugin_id FOR SHARE;
 PERFORM plugin_id FROM webdock_auth.plugin_configuration WHERE plugin_id=j.plugin_id FOR SHARE;
 PERFORM plugin_id FROM webdock_auth.plugin_runtime WHERE plugin_id=j.plugin_id AND generation=j.runtime_generation FOR SHARE;
 PERFORM l.plugin_id FROM webdock_auth.plugin_release l JOIN webdock_auth.plugin_installation p ON p.id=l.plugin_id AND p.version=l.version WHERE p.id=j.plugin_id FOR SHARE OF l;
 PERFORM plugin_id FROM webdock_auth.plugin_grant WHERE plugin_id=j.plugin_id AND customer_id=j.customer_id AND project_id=j.project_id FOR SHARE;
 RETURN true;
END $$;
ALTER FUNCTION webdock_rls.lock_plugin_job_metadata(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.lock_plugin_job_metadata(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.lock_plugin_job_metadata(text) TO webdock_worker_runtime;

CREATE OR REPLACE FUNCTION webdock_rls.plugin_context(capability text) RETURNS jsonb
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE scope jsonb; invocation webdock_auth.plugin_invocation%ROWTYPE; actor jsonb;
 execution webdock_auth.plugin_job_execution%ROWTYPE; parent jsonb; action jsonb; job jsonb; manifest jsonb;
BEGIN
 scope:=webdock_rls.request_scope();
 IF scope IS NULL OR scope->>'pluginInvocationID' IS NULL THEN RETURN NULL; END IF;
 SELECT v.* INTO invocation FROM webdock_auth.plugin_invocation v
 JOIN webdock_auth.plugin_installation p ON p.id=v.plugin_id
 JOIN webdock_auth.plugin_runtime r ON r.plugin_id=p.id AND r.generation=p.current_generation
 JOIN webdock_auth.plugin_configuration config ON config.plugin_id=p.id
 JOIN webdock_auth.plugin_grant g ON g.plugin_id=v.plugin_id AND g.customer_id=v.customer_id AND g.project_id=v.project_id
 JOIN webdock_admin.customers c ON c.id=v.customer_id AND c.status='active'
 JOIN webdock_admin.projects project ON project.id=v.project_id AND project.customer_id=v.customer_id AND project.status='active'
 WHERE v.id::text=scope->>'pluginInvocationID' AND v.revoked_at IS NULL AND v.created_at<=statement_timestamp() AND v.expires_at>statement_timestamp() AND v.expires_at<=statement_timestamp()+interval '60 seconds'
 AND p.state IN ('enabled','draining') AND p.runtime='ready' AND r.status IN ('ready','draining')
 AND p.current_generation=v.runtime_generation AND r.configuration_revision=config.revision
 AND g.revoked_at IS NULL AND g.revision=v.grant_revision
 AND (capability IS NULL OR (config.approved_capabilities ? capability AND g.capabilities ? capability AND v.capabilities ? capability))
 AND (capability IS NULL OR capability IN ('plugin.apps.read','project.read','plugin.data.read','plugin.db.read','plugin.cache.read','plugin.storage.read','plugin.secrets.read','plugin.http.read','plugin.jobs.read') OR v.intent='write')
 AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=project.id AND d.status<>'deleted');
 IF NOT FOUND THEN RETURN NULL; END IF;
 actor:=webdock_rls.actor();
 IF actor IS NULL OR actor->>'subject' IS DISTINCT FROM invocation.actor_id OR actor->>'sessionID' IS DISTINCT FROM invocation.session_id
 OR invocation.session_context->>'customerScope' IS DISTINCT FROM webdock_rls.customer_scope()
 OR (actor->'preview' IS DISTINCT FROM 'null'::jsonb AND webdock_rls.acting_customer() IS DISTINCT FROM invocation.customer_id)
 OR NOT webdock_rls.customer_allowed(invocation.customer_id,invocation.kind='job') THEN RETURN NULL; END IF;
 IF invocation.kind='job' THEN
  SELECT e.* INTO execution FROM webdock_auth.plugin_job_execution e
  JOIN webdock_auth.plugin_grant g ON g.plugin_id=e.plugin_id AND g.customer_id=e.customer_id AND g.project_id=e.project_id
  WHERE e.id=invocation.job_execution_id AND e.plugin_id=invocation.plugin_id AND e.customer_id=invocation.customer_id AND e.project_id=invocation.project_id
  AND e.name=invocation.name AND g.jobs ? e.name AND e.grant_revision=invocation.grant_revision
  AND e.claim_token=invocation.job_claim_token AND e.runtime_generation=invocation.runtime_generation AND e.status='running'
  AND e.invocation_id=invocation.id AND e.lease_until>clock_timestamp() AND NOT e.cancel_requested;
  IF NOT FOUND THEN RETURN NULL; END IF;
  parent:=execution.provenance;
  IF parent IS NULL OR parent->>'kind' IS DISTINCT FROM 'action' OR parent->>'intent' IS DISTINCT FROM 'write'
  OR parent->>'plugin_id' IS DISTINCT FROM invocation.plugin_id OR parent->>'customer_id' IS DISTINCT FROM invocation.customer_id OR parent->>'project_id' IS DISTINCT FROM invocation.project_id
  OR parent->'runtime_generation' IS DISTINCT FROM to_jsonb(invocation.runtime_generation) OR parent->'grant_revision' IS DISTINCT FROM to_jsonb(invocation.grant_revision)
  OR parent->>'actor_id' IS DISTINCT FROM invocation.actor_id OR parent->>'session_id' IS DISTINCT FROM invocation.session_id
  OR parent->'session_context' IS DISTINCT FROM invocation.session_context
  OR parent->'session_context'->'authority' IS NULL OR parent->'session_context'->'authority'='null'::jsonb
  OR parent->'session_context'->'operator' IS DISTINCT FROM to_jsonb(webdock_rls.operator())
  THEN RETURN NULL; END IF;
  SELECT l.manifest_bytes::jsonb INTO manifest FROM webdock_auth.plugin_installation p JOIN webdock_auth.plugin_release l ON l.plugin_id=p.id AND l.version=p.version WHERE p.id=invocation.plugin_id;
  SELECT a INTO action FROM jsonb_array_elements(manifest->'actions') a WHERE a->>'name'=parent->>'name';
  SELECT j INTO job FROM jsonb_array_elements(manifest->'jobs') j WHERE j->>'name'=execution.name;
  IF action IS NULL OR action->>'intent' IS DISTINCT FROM 'write' OR action->'capabilities' IS DISTINCT FROM parent->'capabilities'
   OR NOT action->'capabilities' ? 'plugin.jobs.enqueue' OR job IS NULL OR job ? 'intervalSeconds'
   OR job->'capabilities' IS DISTINCT FROM invocation.capabilities
   OR EXISTS(SELECT 1 FROM jsonb_array_elements_text(job->'capabilities') c WHERE c NOT IN ('plugin.data.read','plugin.data.write','plugin.http.read','plugin.http.request'))
   OR NOT (action->>'audience'='customer' OR action->>'audience'='operator' AND webdock_rls.operator()) THEN RETURN NULL; END IF;
  IF EXISTS(SELECT 1 FROM jsonb_array_elements_text((action->'capabilities')||(job->'capabilities')) cap
   WHERE NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_configuration c JOIN webdock_auth.plugin_grant g ON g.plugin_id=c.plugin_id
    WHERE c.plugin_id=invocation.plugin_id AND g.customer_id=invocation.customer_id AND g.project_id=invocation.project_id
    AND c.approved_capabilities ? cap AND g.capabilities ? cap)) THEN RETURN NULL; END IF;
  IF action ? 'feature' AND (NOT webdock_rls.plugin_feature_supported(invocation.plugin_id,action->>'feature') OR
   NOT webdock_rls.operator() AND NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_feature_policy p WHERE p.plugin_id=invocation.plugin_id AND p.feature_id=action->>'feature'
    AND (p.mode='everyone' OR p.mode='selected' AND EXISTS(SELECT 1 FROM webdock_auth.plugin_feature_customer c WHERE c.plugin_id=p.plugin_id AND c.feature_id=p.feature_id AND c.customer_id=invocation.customer_id)))) THEN RETURN NULL; END IF;
  IF action->>'resource' IN ('use','delete') AND NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_resource r WHERE r.plugin_id=invocation.plugin_id AND r.project_id=invocation.project_id AND r.customer_id=invocation.customer_id AND r.resource_id=parent->'session_context'->>'resourceID') THEN RETURN NULL; END IF;
 ELSIF invocation.kind<>'action' THEN RETURN NULL;
 END IF;
 RETURN jsonb_build_object('pluginID',invocation.plugin_id,'customerID',invocation.customer_id,'projectID',invocation.project_id,'operationID',invocation.operation_id);
END $$;

CREATE FUNCTION webdock_rls.plugin_job_enqueue(job_id text,job_name text,idem text,payload jsonb,maximum integer,fingerprint text) RETURNS jsonb
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE scope jsonb; v webdock_auth.plugin_invocation%ROWTYPE; existing webdock_auth.plugin_job_execution%ROWTYPE;
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
 INSERT INTO webdock_auth.plugin_job_execution(id,plugin_id,customer_id,project_id,name,idempotency_key,payload,grant_revision,runtime_generation,provenance,request_hash,max_attempts)
 VALUES(job_id,v.plugin_id,v.customer_id,v.project_id,job_name,idem,payload,v.grant_revision,v.runtime_generation,to_jsonb(v),fingerprint,maximum) ON CONFLICT DO NOTHING;
 SELECT * INTO existing FROM webdock_auth.plugin_job_execution WHERE plugin_id=v.plugin_id AND customer_id=v.customer_id AND project_id=v.project_id AND idempotency_key=idem;
 IF NOT FOUND THEN RETURN NULL; END IF;
 IF existing.request_hash IS DISTINCT FROM fingerprint THEN RETURN '{"conflict":true}'::jsonb; END IF;
 RETURN jsonb_build_object('id',existing.id);
END $$;

CREATE FUNCTION webdock_rls.plugin_job_get(job_id text,capability text) RETURNS jsonb
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE scope jsonb; j webdock_auth.plugin_job_execution%ROWTYPE;
BEGIN
 IF capability IS NULL OR capability NOT IN ('plugin.jobs.read','plugin.jobs.cancel') OR NOT webdock_rls.lock_plugin_invocation() THEN RETURN NULL; END IF;
 scope:=webdock_rls.plugin_context(capability);
 IF scope IS NULL THEN RETURN NULL; END IF;
 SELECT e.* INTO j FROM webdock_auth.plugin_job_execution e JOIN webdock_auth.plugin_grant g USING(plugin_id,customer_id,project_id)
 WHERE e.id=job_id AND e.plugin_id=scope->>'pluginID' AND e.customer_id=scope->>'customerID' AND e.project_id=scope->>'projectID' AND g.jobs ? e.name;
 IF NOT FOUND THEN RETURN NULL; END IF;
 RETURN jsonb_build_object('id',j.id,'name',j.name,'state',CASE j.status WHEN 'pending' THEN 'waiting' WHEN 'running' THEN 'active' WHEN 'succeeded' THEN 'completed' ELSE j.status END,'attempts',j.attempts)
 || CASE WHEN j.status='succeeded' THEN jsonb_build_object('result',j.result) ELSE '{}'::jsonb END;
END $$;
CREATE FUNCTION webdock_rls.plugin_job_cancel(job_id text) RETURNS jsonb
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE visible jsonb; j webdock_auth.plugin_job_execution%ROWTYPE;
BEGIN
 visible:=webdock_rls.plugin_job_get(job_id,'plugin.jobs.cancel');
 IF visible IS NULL THEN RETURN NULL; END IF;
 SELECT * INTO j FROM webdock_auth.plugin_job_execution WHERE id=job_id FOR UPDATE;
 IF j.status IN ('succeeded','failed') THEN RETURN '{"conflict":true}'::jsonb; END IF;
 UPDATE webdock_auth.plugin_job_execution SET cancel_requested=true,status='cancelled',claim_token=NULL,lease_until=NULL,condition='cancelled' WHERE id=job_id;
 UPDATE webdock_auth.plugin_invocation SET revoked_at=coalesce(revoked_at,now()) WHERE id=j.invocation_id;
 RETURN jsonb_build_object('id',job_id,'cancelRequested',true);
END $$;
DO $$ DECLARE f text; BEGIN
 FOREACH f IN ARRAY ARRAY['plugin_job_enqueue(text,text,text,jsonb,integer,text)','plugin_job_get(text,text)','plugin_job_cancel(text)'] LOOP
  EXECUTE 'ALTER FUNCTION webdock_rls.'||f||' OWNER TO webdock_platform_owner';
  EXECUTE 'REVOKE ALL ON FUNCTION webdock_rls.'||f||' FROM PUBLIC';
  EXECUTE 'GRANT EXECUTE ON FUNCTION webdock_rls.'||f||' TO webdock_auth_runtime';
 END LOOP;
END $$;
