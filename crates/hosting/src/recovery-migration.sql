-- Draft for reserved0052. Moved into the migration journal only after compiler hold release.
-- Installation-wide Mail sets are operator-only: no invented tenant or browser session.
CREATE TABLE webdock_auth.native_recovery_epoch (
 singleton boolean PRIMARY KEY CHECK(singleton),
 epoch uuid NOT NULL, state text NOT NULL CHECK(state IN('active','fencing','fenced','quarantined')),
 operation_id varchar REFERENCES webdock_auth.hosting_operation(id),
 generation integer NOT NULL DEFAULT 0, intent_revision bigint NOT NULL DEFAULT 1,
 source_resume_allowed boolean NOT NULL DEFAULT false
);
INSERT INTO webdock_auth.native_recovery_epoch VALUES(true,gen_random_uuid(),'active',NULL,0,1,false);
CREATE FUNCTION webdock_rls.recovery_target_quarantined() RETURNS boolean
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
 SELECT EXISTS(SELECT 1 FROM webdock_auth.native_recovery_epoch WHERE singleton AND state='quarantined')
$$;
ALTER FUNCTION webdock_rls.recovery_target_quarantined() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.recovery_target_quarantined() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.recovery_target_quarantined() TO webdock_policy_owner,webdock_auth_runtime,webdock_worker_runtime,webdock_auth_bootstrap,webdock_registry_runtime;
-- Preserve function identity and every retained worker-purpose/cluster rule. A cold
-- target rejects all prior system execution authority, including unrelated queues.
CREATE OR REPLACE FUNCTION webdock_rls.worker(purposes text[]) RETURNS boolean
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
 SELECT coalesce(session_user='webdock_worker_runtime' AND webdock_rls.request_scope()->>'kind'='system'
  AND webdock_rls.request_scope()->>'purpose'=ANY(purposes),false)
  AND NOT webdock_rls.recovery_target_quarantined()
$$;
-- The actor implementation contains retained Clerk/local preview semantics. Add one
-- early quarantine denial without dropping/replacing its OID or rebuilding its body.
DO $$ DECLARE definition text; BEGIN
 SELECT pg_get_functiondef('webdock_rls.actor()'::regprocedure) INTO definition;
 IF position(E'\nBEGIN\n' IN definition)=0 THEN RAISE EXCEPTION 'Actor quarantine insertion point changed'; END IF;
 EXECUTE regexp_replace(definition,E'\nBEGIN\n',E'\nBEGIN\n IF webdock_rls.recovery_target_quarantined() THEN RETURN NULL; END IF;\n');
END $$;
CREATE TABLE webdock_auth.hosting_recovery_policy (
 id varchar PRIMARY KEY DEFAULT webdock_auth.next_snowflake(),
 cluster_id varchar NOT NULL REFERENCES webdock_auth.hosting_cluster(id),
 profile text NOT NULL CHECK(profile IN('mail-installation-v1','ntfy-sqlite-offline-v1')),
 source_app_id varchar REFERENCES webdock_auth.hosting_app(id),
 source jsonb NOT NULL CHECK(jsonb_typeof(source)='object' AND octet_length(source::text)<=32768),
 destination jsonb NOT NULL CHECK(jsonb_typeof(destination)='object' AND octet_length(destination::text)<=16384),
 capacity_bytes bigint NOT NULL CHECK(capacity_bytes BETWEEN 67108864 AND 68719476736),
 interval_seconds integer NOT NULL CHECK(interval_seconds BETWEEN 60 AND 2592000),
 approved_by text NOT NULL REFERENCES webdock_auth."user"(id),
 approved_session text NOT NULL,
 revision integer NOT NULL DEFAULT 1 CHECK(revision>0),
 created_at timestamptz NOT NULL DEFAULT now(), revoked_at timestamptz,
 next_run_at timestamptz NOT NULL DEFAULT now(), last_success_at timestamptz,
 last_error text, observed_status text NOT NULL DEFAULT 'pending',
 CHECK((profile='mail-installation-v1' AND source_app_id IS NULL) OR (profile='ntfy-sqlite-offline-v1' AND source_app_id IS NOT NULL))
);
CREATE UNIQUE INDEX one_live_mail_recovery_policy ON webdock_auth.hosting_recovery_policy(profile) WHERE profile='mail-installation-v1' AND revoked_at IS NULL;
CREATE UNIQUE INDEX one_live_app_recovery_policy ON webdock_auth.hosting_recovery_policy(source_app_id) WHERE revoked_at IS NULL;
CREATE TABLE webdock_auth.hosting_recovery_job (
 operation_id varchar PRIMARY KEY REFERENCES webdock_auth.hosting_operation(id),
 policy_id varchar NOT NULL REFERENCES webdock_auth.hosting_recovery_policy(id),
 policy_revision integer NOT NULL,
 spec jsonb NOT NULL CHECK(octet_length(spec::text)<=65536),
 lease_hash text, checkpoint jsonb CHECK(octet_length(checkpoint::text)<=65536),
 request_sha256 text, completed_proof jsonb CHECK(octet_length(completed_proof::text)<=65536),
 source_fence_epoch uuid, created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE webdock_auth.hosting_recovery_set (
 id varchar PRIMARY KEY DEFAULT webdock_auth.next_snowflake(),
 operation_id varchar NOT NULL UNIQUE REFERENCES webdock_auth.hosting_operation(id),
 policy_id varchar NOT NULL REFERENCES webdock_auth.hosting_recovery_policy(id),
 policy_revision integer NOT NULL, epoch uuid NOT NULL,
 proof jsonb NOT NULL CHECK(octet_length(proof::text)<=65536),
 completed_at timestamptz NOT NULL DEFAULT now(), referenced boolean NOT NULL DEFAULT true
);

CREATE FUNCTION webdock_rls.recovery_policy_live(policy text,cluster text) RETURNS boolean
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
 SELECT webdock_rls.worker(ARRAY['agent'],cluster) AND EXISTS(
  SELECT 1 FROM webdock_auth.hosting_recovery_policy p
  JOIN webdock_auth."user" u ON u.id=p.approved_by
  JOIN webdock_auth.hosting_cluster cl ON cl.id=p.cluster_id
  JOIN webdock_auth.hosting_agent a ON a.cluster_id=cl.id
  WHERE p.id=policy AND p.cluster_id=cluster AND p.revoked_at IS NULL
   AND u.role='operator' AND u."twoFactorEnabled" AND u."emailVerified"
   AND NOT coalesce(u.banned,false) AND NOT u."mustChangePassword"
   AND cl.ownership='platform' AND cl.verified AND NOT a.revoked
   AND EXISTS(SELECT 1 FROM webdock_auth.plugin_installation i WHERE i.id='hosting' AND i.state='enabled')
   AND ((p.profile='mail-installation-v1'
     AND EXISTS(SELECT 1 FROM webdock_auth.plugin_installation i WHERE i.id='mail' AND i.state IN('enabled','draining')))
    OR (p.profile='ntfy-sqlite-offline-v1' AND EXISTS(
     SELECT 1 FROM webdock_auth.hosting_app app
     JOIN webdock_auth.hosting_project hp ON hp.project_id=app.project_id
     JOIN webdock_admin.projects pr ON pr.id=app.project_id
     JOIN webdock_admin.customers c ON c.id=pr.customer_id
     WHERE app.id=p.source_app_id AND hp.cluster_id=cluster AND pr.status='active' AND c.status='active'
      AND app.revision=(p.source->>'revision')::integer AND app.spec=p.source->'spec'
      AND app.observed_uid=p.source->>'runtimeUID' AND app.status='ready'
      AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=pr.id AND d.status<>'deleted')))))
$$;
ALTER FUNCTION webdock_rls.recovery_policy_live(text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.recovery_policy_live(text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.recovery_policy_live(text,text) TO webdock_worker_runtime;

CREATE FUNCTION webdock_rls.current_recovery_epoch() RETURNS uuid
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
 SELECT epoch FROM webdock_auth.native_recovery_epoch WHERE singleton
$$;
ALTER FUNCTION webdock_rls.current_recovery_epoch() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.current_recovery_epoch() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.current_recovery_epoch() TO webdock_worker_runtime,webdock_auth_runtime;

CREATE FUNCTION webdock_rls.recovery_immutable() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 IF TG_TABLE_NAME='hosting_recovery_policy' THEN
  IF (NEW.id,NEW.cluster_id,NEW.profile,NEW.source_app_id,NEW.source,NEW.destination,NEW.capacity_bytes,NEW.approved_by,NEW.approved_session,NEW.created_at,NEW.interval_seconds)
   IS DISTINCT FROM (OLD.id,OLD.cluster_id,OLD.profile,OLD.source_app_id,OLD.source,OLD.destination,OLD.capacity_bytes,OLD.approved_by,OLD.approved_session,OLD.created_at,OLD.interval_seconds)
   OR (OLD.revoked_at IS NOT NULL AND NEW.revoked_at IS DISTINCT FROM OLD.revoked_at)
   OR NEW.revision NOT BETWEEN OLD.revision AND OLD.revision+1
   OR (NEW.revoked_at IS DISTINCT FROM OLD.revoked_at AND NEW.revision<>OLD.revision+1)
   OR (current_user='webdock_worker_runtime' AND (NEW.revoked_at,NEW.revision) IS DISTINCT FROM (OLD.revoked_at,OLD.revision))
   THEN RAISE EXCEPTION 'Immutable recovery authority' USING ERRCODE='42501';END IF;
 ELSE RAISE EXCEPTION 'Immutable referenced recovery set' USING ERRCODE='42501'; END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER recovery_policy_immutable BEFORE UPDATE ON webdock_auth.hosting_recovery_policy FOR EACH ROW EXECUTE FUNCTION webdock_rls.recovery_immutable();
CREATE TRIGGER recovery_set_immutable BEFORE UPDATE OR DELETE ON webdock_auth.hosting_recovery_set FOR EACH ROW EXECUTE FUNCTION webdock_rls.recovery_immutable();
DO $$ DECLARE t text; BEGIN
 FOREACH t IN ARRAY ARRAY['native_recovery_epoch','hosting_recovery_policy','hosting_recovery_job','hosting_recovery_set'] LOOP
  EXECUTE format('ALTER TABLE webdock_auth.%I OWNER TO webdock_platform_owner',t);
  EXECUTE format('ALTER TABLE webdock_auth.%I ENABLE ROW LEVEL SECURITY',t);
  EXECUTE format('ALTER TABLE webdock_auth.%I FORCE ROW LEVEL SECURITY',t);
  EXECUTE format('CREATE POLICY platform_owner ON webdock_auth.%I TO webdock_platform_owner USING(true) WITH CHECK(true)',t);
 END LOOP;
 FOREACH t IN ARRAY ARRAY['hosting_recovery_policy','hosting_recovery_job','hosting_recovery_set'] LOOP
  EXECUTE format('GRANT SELECT,INSERT,UPDATE ON webdock_auth.%I TO webdock_auth_runtime,webdock_worker_runtime',t);
  EXECUTE format('CREATE POLICY recovery_operator ON webdock_auth.%I TO webdock_auth_runtime USING(webdock_rls.operator() AND webdock_rls.actor()->''preview''=''null''::jsonb) WITH CHECK(webdock_rls.operator() AND webdock_rls.actor()->''preview''=''null''::jsonb)',t);
 END LOOP;
END $$;
CREATE POLICY recovery_agent_policy ON webdock_auth.hosting_recovery_policy TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['agent'],cluster_id)) WITH CHECK(webdock_rls.worker(ARRAY['agent'],cluster_id));
CREATE POLICY recovery_agent_job ON webdock_auth.hosting_recovery_job TO webdock_worker_runtime USING(EXISTS(SELECT 1 FROM webdock_auth.hosting_recovery_policy p WHERE p.id=policy_id AND webdock_rls.worker(ARRAY['agent'],p.cluster_id))) WITH CHECK(EXISTS(SELECT 1 FROM webdock_auth.hosting_recovery_policy p WHERE p.id=policy_id AND webdock_rls.worker(ARRAY['agent'],p.cluster_id)));
CREATE POLICY recovery_agent_set ON webdock_auth.hosting_recovery_set TO webdock_worker_runtime USING(EXISTS(SELECT 1 FROM webdock_auth.hosting_recovery_policy p WHERE p.id=policy_id AND webdock_rls.worker(ARRAY['agent'],p.cluster_id))) WITH CHECK(EXISTS(SELECT 1 FROM webdock_auth.hosting_recovery_policy p WHERE p.id=policy_id AND webdock_rls.worker(ARRAY['agent'],p.cluster_id)));

CREATE FUNCTION webdock_rls.mail_recovery_open() RETURNS boolean
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 PERFORM singleton FROM webdock_auth.native_recovery_epoch WHERE singleton AND state='active' FOR SHARE;
 RETURN FOUND;
END $$;
ALTER FUNCTION webdock_rls.mail_recovery_open() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.mail_recovery_open() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.mail_recovery_open() TO webdock_worker_runtime,webdock_auth_runtime;
CREATE FUNCTION webdock_rls.mail_recovery_mutation_guard() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 -- Owner-only recovery functions may close/restore; no runtime GUC bypass exists.
 IF current_user='webdock_platform_owner' THEN RETURN NULL; END IF;
 IF NOT webdock_rls.mail_recovery_open() THEN RAISE EXCEPTION 'Mail recovery fenced' USING ERRCODE='55000'; END IF;
 RETURN NULL;
END $$;
DO $$ DECLARE r record; BEGIN
 FOR r IN SELECT n.nspname,c.relname FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE c.relkind='r' AND
  (n.nspname='webdock_mail' OR n.nspname='webdock_auth' AND c.relname LIKE 'mail\_%' ESCAPE '\') LOOP
  EXECUTE format('CREATE TRIGGER native_recovery_fence BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON %I.%I FOR EACH STATEMENT EXECUTE FUNCTION webdock_rls.mail_recovery_mutation_guard()',r.nspname,r.relname);
 END LOOP;
END $$;

CREATE FUNCTION webdock_rls.recovery_mail_fence(operation text,generation integer,token_hash text,finish boolean DEFAULT false) RETURNS uuid
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE j webdock_auth.hosting_recovery_job%ROWTYPE;p webdock_auth.hosting_recovery_policy%ROWTYPE;e webdock_auth.native_recovery_epoch%ROWTYPE;
BEGIN
 SELECT * INTO j FROM webdock_auth.hosting_recovery_job WHERE operation_id=operation;
 SELECT * INTO p FROM webdock_auth.hosting_recovery_policy WHERE id=j.policy_id;
 IF p.profile<>'mail-installation-v1' OR NOT webdock_rls.recovery_policy_live(p.id,p.cluster_id)
  OR j.policy_revision<>p.revision OR j.lease_hash IS DISTINCT FROM token_hash
  OR NOT EXISTS(SELECT 1 FROM webdock_auth.hosting_operation o WHERE o.id=operation AND o.status='running' AND o.generation=recovery_mail_fence.generation AND o.lease_until>clock_timestamp()) THEN
  RAISE EXCEPTION 'Current recovery lease required' USING ERRCODE='42501'; END IF;
 SELECT * INTO e FROM webdock_auth.native_recovery_epoch WHERE singleton FOR UPDATE;
 IF e.state='quarantined' OR e.state<>'active' AND (e.operation_id,e.generation) IS DISTINCT FROM (operation,generation) THEN
  RAISE EXCEPTION 'Owned recovery epoch required' USING ERRCODE='42501'; END IF;
 IF e.state='active' THEN
  UPDATE webdock_auth.native_recovery_epoch SET state='fencing',operation_id=operation,generation=recovery_mail_fence.generation,intent_revision=p.revision,source_resume_allowed=true WHERE singleton;
 END IF;
 IF finish THEN
  -- Actual executors/CE writers must already be observed stopped. Lost handoffs are
  -- retained as unknown in the one canonical ledger; nothing is resent or settled.
  UPDATE webdock_mail.send_job SET state='unknown',finished_at=NULL WHERE state='handoff';
  UPDATE webdock_auth.native_recovery_epoch SET state='fenced' WHERE singleton;
 END IF;
 RETURN e.epoch;
END $$;
ALTER FUNCTION webdock_rls.recovery_mail_fence(text,integer,text,boolean) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.recovery_mail_fence(text,integer,text,boolean) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.recovery_mail_fence(text,integer,text,boolean) TO webdock_worker_runtime;

CREATE FUNCTION webdock_rls.recovery_source_resume(operation text,generation integer,token_hash text,expected_epoch uuid) RETURNS boolean
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM webdock_auth.hosting_recovery_job j JOIN webdock_auth.hosting_recovery_policy p ON p.id=j.policy_id JOIN webdock_auth.hosting_operation o ON o.id=j.operation_id
  WHERE o.id=operation AND o.generation=recovery_source_resume.generation AND o.status='running' AND o.lease_until>clock_timestamp()
   AND j.lease_hash=token_hash AND j.policy_revision=p.revision AND webdock_rls.recovery_policy_live(p.id,p.cluster_id)) THEN RETURN false; END IF;
 UPDATE webdock_auth.native_recovery_epoch SET state='active',operation_id=NULL,source_resume_allowed=false
  WHERE singleton AND state='fenced' AND epoch=expected_epoch AND operation_id=operation AND native_recovery_epoch.generation=recovery_source_resume.generation
   AND source_resume_allowed AND intent_revision=(SELECT policy_revision FROM webdock_auth.hosting_recovery_job WHERE operation_id=operation);
 RETURN FOUND;
END $$;
ALTER FUNCTION webdock_rls.recovery_source_resume(text,integer,text,uuid) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.recovery_source_resume(text,integer,text,uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.recovery_source_resume(text,integer,text,uuid) TO webdock_worker_runtime;

-- Offline target-only primitive. Not executable by any runtime or customer role.
-- Call before starting restored listeners/controllers, then keep target networking closed.
CREATE FUNCTION webdock_auth.quarantine_fresh_recovery_target(expected_epoch uuid) RETURNS uuid
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE fresh uuid:=gen_random_uuid();
BEGIN
 IF NOT EXISTS(SELECT 1 FROM webdock_auth.native_recovery_epoch WHERE singleton AND state='fenced' AND epoch=expected_epoch FOR UPDATE) THEN
  RAISE EXCEPTION 'Matched offline recovery target required' USING ERRCODE='55000'; END IF;
 UPDATE webdock_auth.native_recovery_epoch SET state='quarantined',epoch=fresh,source_resume_allowed=false,intent_revision=intent_revision+1 WHERE singleton;
 UPDATE webdock_auth.session SET "expiresAt"=clock_timestamp()-interval '1 second';
 UPDATE webdock_auth.api_token SET expires_at=clock_timestamp()-interval '1 second';
 UPDATE webdock_auth."oauthAccessToken" SET revoked=coalesce(revoked,clock_timestamp()),"expiresAt"=clock_timestamp()-interval '1 second';
 UPDATE webdock_auth."oauthRefreshToken" SET revoked=coalesce(revoked,clock_timestamp()),"expiresAt"=clock_timestamp()-interval '1 second';
 UPDATE webdock_auth.hosting_agent SET revoked=true,generation=generation+1;
 UPDATE webdock_auth.agent_transport_enrollment SET revoked=true,generation=generation+1;
 UPDATE webdock_auth.hosting_enrollment SET token_hash=NULL,expires_at=clock_timestamp()-interval '1 second';
 UPDATE webdock_auth.plugin_runtime SET credential_hash=NULL,status='stopped';
 UPDATE webdock_mail.api_key SET revoked_at=coalesce(revoked_at,clock_timestamp());
 UPDATE webdock_auth.mail_api_key SET revoked_at=coalesce(revoked_at,clock_timestamp());
 UPDATE webdock_mail.webmail_session SET expires_at=clock_timestamp()-interval '1 second';
 UPDATE webdock_mail.webmail_launch SET expires_at=clock_timestamp()-interval '1 second';
 UPDATE webdock_auth.hosting_recovery_policy SET revoked_at=coalesce(revoked_at,clock_timestamp()),revision=revision+CASE WHEN revoked_at IS NULL THEN 1 ELSE 0 END;
 RETURN fresh;
END $$;
ALTER FUNCTION webdock_auth.quarantine_fresh_recovery_target(uuid) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.quarantine_fresh_recovery_target(uuid) FROM PUBLIC;
