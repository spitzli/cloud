-- Additive, private Kubernetes execution. No existing source or worker is opted in.
ALTER TABLE webdock_auth.git_source ADD COLUMN execution_backend text NOT NULL DEFAULT 'microvm'
 CHECK (execution_backend IN ('microvm','kubernetes'));
ALTER TABLE webdock_auth.git_source ADD CONSTRAINT git_source_kubernetes_recipe CHECK (
 execution_backend<>'kubernetes' OR (build_provider='isolated' AND recipe IN ('dockerfile','dockerfile-frontend')));
ALTER TABLE webdock_auth.git_build ADD COLUMN execution_backend text NOT NULL DEFAULT 'microvm'
 CHECK (execution_backend IN ('microvm','kubernetes'));

CREATE TABLE webdock_auth.git_build_admission (
 singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
 enabled boolean NOT NULL DEFAULT false,
 global_slots integer NOT NULL DEFAULT 1 CHECK(global_slots BETWEEN 1 AND 256),
 tenant_slots integer NOT NULL DEFAULT 1 CHECK(tenant_slots BETWEEN 1 AND 32)
);
INSERT INTO webdock_auth.git_build_admission(singleton) VALUES(true);
CREATE TABLE webdock_auth.git_build_trusted_project (
 project_id varchar PRIMARY KEY REFERENCES webdock_admin.projects(id),
 customer_id varchar NOT NULL REFERENCES webdock_admin.customers(id),
 enabled boolean NOT NULL DEFAULT false,
 updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE webdock_auth.git_build_cluster (
 id varchar PRIMARY KEY,
 worker_id varchar NOT NULL UNIQUE REFERENCES webdock_auth.git_worker(id),
 worker_generation integer NOT NULL CHECK(worker_generation>0),
 namespace text NOT NULL CHECK(namespace ~ '^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$'),
 country text NOT NULL CHECK(country IN ('AT','BE','BG','HR','CY','CZ','DE','DK','EE','ES','FI','FR','GR','HU','IE','IT','LT','LU','LV','MT','NL','PL','PT','RO','SE','SI','SK')),
 evidence text NOT NULL CHECK(octet_length(evidence) BETWEEN 20 AND 4000),
 capacity integer NOT NULL CHECK(capacity BETWEEN 1 AND 32),
 enabled boolean NOT NULL DEFAULT false,
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE webdock_auth.git_build_attempt (
 id varchar PRIMARY KEY,
 build_id varchar NOT NULL REFERENCES webdock_auth.git_build(id),
 customer_id varchar NOT NULL REFERENCES webdock_admin.customers(id),
 project_id varchar NOT NULL REFERENCES webdock_admin.projects(id),
 generation integer NOT NULL CHECK(generation>0),
 worker_id varchar NOT NULL REFERENCES webdock_auth.git_worker(id),
 worker_generation integer NOT NULL CHECK(worker_generation>0),
 cluster_id varchar NOT NULL REFERENCES webdock_auth.git_build_cluster(id),
 namespace text NOT NULL,
 job_name text NOT NULL CHECK(job_name ~ '^wd-build-[a-f0-9]{40}-[1-9][0-9]{0,9}$'),
 execution_request jsonb NOT NULL CHECK (
  jsonb_typeof(execution_request)='object' AND octet_length(execution_request::text)<=16384
  AND execution_request ?& ARRAY['recipe','rootDirectory','limits']
  AND execution_request-ARRAY['recipe','rootDirectory','limits']='{}'::jsonb
  AND execution_request->>'recipe' IN ('dockerfile','dockerfile-frontend')
  AND jsonb_typeof(execution_request->'rootDirectory')='string'
  AND jsonb_typeof(execution_request->'limits')='object'
  AND (execution_request->'limits') ?& ARRAY['cpu','memoryMiB','diskBytes','durationSeconds']
  AND (execution_request->'limits')-ARRAY['cpu','memoryMiB','diskBytes','durationSeconds']='{}'::jsonb
  AND jsonb_typeof(execution_request#>'{limits,cpu}')='number'
  AND jsonb_typeof(execution_request#>'{limits,memoryMiB}')='number'
  AND jsonb_typeof(execution_request#>'{limits,diskBytes}')='number'
  AND jsonb_typeof(execution_request#>'{limits,durationSeconds}')='number'
  AND (execution_request#>>'{limits,cpu}')::numeric BETWEEN 1 AND 8
  AND (execution_request#>>'{limits,memoryMiB}')::numeric BETWEEN 512 AND 16384
  AND (execution_request#>>'{limits,diskBytes}')::numeric BETWEEN 10000000 AND 34359738368
  AND (execution_request#>>'{limits,durationSeconds}')::numeric BETWEEN 30 AND 1800
 ),
 runtime_fingerprint text NOT NULL CHECK(runtime_fingerprint ~ '^[a-f0-9]{64}$'),
 spec_fingerprint text CHECK(spec_fingerprint ~ '^[a-f0-9]{64}$'),
 job_uid text CHECK(octet_length(job_uid) BETWEEN 1 AND 128 AND job_uid ~ '^[A-Za-z0-9_-]+$'),
 cancel_requested boolean NOT NULL DEFAULT false,
 created_at timestamptz NOT NULL DEFAULT now(),
 retired_at timestamptz,
 UNIQUE(build_id,generation),
 UNIQUE(cluster_id,namespace,job_name),
 CHECK(job_uid IS NULL OR spec_fingerprint IS NOT NULL)
);
-- Lease expiry is deliberately absent: uncertain Jobs continue to consume capacity.
CREATE UNIQUE INDEX git_build_attempt_one_live ON webdock_auth.git_build_attempt(build_id) WHERE retired_at IS NULL;
CREATE INDEX git_build_attempt_capacity ON webdock_auth.git_build_attempt(cluster_id,customer_id) WHERE retired_at IS NULL;

DO $$ DECLARE name text; BEGIN
 FOREACH name IN ARRAY ARRAY['git_build_admission','git_build_trusted_project','git_build_cluster','git_build_attempt'] LOOP
  EXECUTE format('ALTER TABLE webdock_auth.%I OWNER TO webdock_platform_owner',name);
  EXECUTE format('ALTER TABLE webdock_auth.%I ENABLE ROW LEVEL SECURITY',name);
  EXECUTE format('ALTER TABLE webdock_auth.%I FORCE ROW LEVEL SECURITY',name);
  EXECUTE format('CREATE POLICY owner_internal ON webdock_auth.%I TO webdock_platform_owner USING(true) WITH CHECK(true)',name);
  EXECUTE format('CREATE POLICY worker_read ON webdock_auth.%I FOR SELECT TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY[''git-worker'']))',name);
  EXECUTE format('CREATE POLICY operator_read ON webdock_auth.%I FOR SELECT TO webdock_auth_runtime USING(webdock_rls.operator())',name);
  EXECUTE format('GRANT SELECT ON webdock_auth.%I TO webdock_auth_runtime,webdock_worker_runtime',name);
 END LOOP;
 FOREACH name IN ARRAY ARRAY['git_build_admission','git_build_trusted_project','git_build_cluster'] LOOP
  EXECUTE format('CREATE POLICY operator_insert ON webdock_auth.%I FOR INSERT TO webdock_auth_runtime WITH CHECK(webdock_rls.operator())',name);
  EXECUTE format('CREATE POLICY operator_update ON webdock_auth.%I FOR UPDATE TO webdock_auth_runtime USING(webdock_rls.operator()) WITH CHECK(webdock_rls.operator())',name);
  EXECUTE format('GRANT INSERT,UPDATE ON webdock_auth.%I TO webdock_auth_runtime',name);
 END LOOP;
END $$;
CREATE POLICY tenant_read ON webdock_auth.git_build_attempt FOR SELECT TO webdock_auth_runtime
 USING(webdock_rls.customer_allowed(customer_id) AND webdock_rls.project_matches(project_id,customer_id));
CREATE POLICY source_selection_read ON webdock_auth.git_build_admission FOR SELECT TO webdock_auth_runtime
 USING(webdock_rls.actor() IS NOT NULL);
CREATE POLICY source_selection_read ON webdock_auth.git_build_trusted_project FOR SELECT TO webdock_auth_runtime
 USING(webdock_rls.customer_allowed(customer_id) AND webdock_rls.project_matches(project_id,customer_id));
CREATE POLICY worker_insert ON webdock_auth.git_build_attempt FOR INSERT TO webdock_worker_runtime
 WITH CHECK(webdock_rls.worker(ARRAY['git-worker']));
CREATE POLICY worker_update ON webdock_auth.git_build_attempt FOR UPDATE TO webdock_worker_runtime
 USING(webdock_rls.worker(ARRAY['git-worker'])) WITH CHECK(webdock_rls.worker(ARRAY['git-worker']));
GRANT INSERT,UPDATE ON webdock_auth.git_build_attempt TO webdock_worker_runtime;

CREATE FUNCTION webdock_rls.guard_git_build_backend() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 IF TG_TABLE_NAME='git_source' THEN
  IF NEW.execution_backend IS DISTINCT FROM OLD.execution_backend AND NEW.revision<=OLD.revision THEN
   RAISE EXCEPTION 'backend change requires source revision' USING ERRCODE='23514';
  END IF;
 ELSIF TG_OP='INSERT' THEN
  SELECT s.execution_backend INTO NEW.execution_backend FROM webdock_auth.git_source s
   WHERE s.id=NEW.source_id AND s.customer_id=NEW.customer_id AND s.project_id=NEW.project_id
    AND s.revision=NEW.source_revision AND s.environment_revision=NEW.environment_revision;
  IF NEW.execution_backend IS NULL THEN RAISE EXCEPTION 'source revision changed' USING ERRCODE='23514'; END IF;
 ELSIF NEW.execution_backend IS DISTINCT FROM OLD.execution_backend THEN
  RAISE EXCEPTION 'build backend is immutable' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_rls.guard_git_build_backend() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.guard_git_build_backend() FROM PUBLIC;
CREATE TRIGGER git_source_backend BEFORE UPDATE ON webdock_auth.git_source FOR EACH ROW EXECUTE FUNCTION webdock_rls.guard_git_build_backend();
CREATE TRIGGER git_build_backend BEFORE INSERT OR UPDATE ON webdock_auth.git_build FOR EACH ROW EXECUTE FUNCTION webdock_rls.guard_git_build_backend();

CREATE FUNCTION webdock_rls.guard_git_build_attempt() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 IF TG_OP='INSERT' THEN
  IF NEW.retired_at IS NOT NULL OR NEW.cancel_requested OR NEW.spec_fingerprint IS NOT NULL OR NEW.job_uid IS NOT NULL
   OR NEW.job_name<>('wd-build-'||substring(encode(sha256(convert_to(NEW.build_id,'UTF8')),'hex') FROM 1 FOR 40)||'-'||NEW.generation::text)
   OR NOT EXISTS(SELECT 1 FROM webdock_auth.git_build b JOIN webdock_auth.git_build_cluster c ON c.id=NEW.cluster_id
   WHERE b.id=NEW.build_id AND b.customer_id=NEW.customer_id AND b.project_id=NEW.project_id
    AND b.execution_backend='kubernetes' AND b.generation=NEW.generation AND b.status='running'
    AND b.worker_id=NEW.worker_id AND b.worker_generation=NEW.worker_generation
    AND c.worker_id=NEW.worker_id AND c.worker_generation=NEW.worker_generation AND c.namespace=NEW.namespace) THEN
   RAISE EXCEPTION 'attempt scope mismatch' USING ERRCODE='23514';
  END IF;
 ELSE
  IF ROW(NEW.id,NEW.build_id,NEW.customer_id,NEW.project_id,NEW.generation,NEW.worker_id,NEW.worker_generation,
    NEW.cluster_id,NEW.namespace,NEW.job_name,NEW.execution_request,NEW.runtime_fingerprint,NEW.created_at)
   IS DISTINCT FROM ROW(OLD.id,OLD.build_id,OLD.customer_id,OLD.project_id,OLD.generation,OLD.worker_id,OLD.worker_generation,
    OLD.cluster_id,OLD.namespace,OLD.job_name,OLD.execution_request,OLD.runtime_fingerprint,OLD.created_at)
   OR (OLD.spec_fingerprint IS NOT NULL AND NEW.spec_fingerprint IS DISTINCT FROM OLD.spec_fingerprint)
   OR (OLD.job_uid IS NOT NULL AND NEW.job_uid IS DISTINCT FROM OLD.job_uid)
   OR (OLD.cancel_requested AND NOT NEW.cancel_requested)
   OR (OLD.retired_at IS NOT NULL AND NEW IS DISTINCT FROM OLD) THEN
   RAISE EXCEPTION 'attempt identity is immutable' USING ERRCODE='23514';
  END IF;
 END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_rls.guard_git_build_attempt() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.guard_git_build_attempt() FROM PUBLIC;
CREATE TRIGGER git_build_attempt_guard BEFORE INSERT OR UPDATE ON webdock_auth.git_build_attempt FOR EACH ROW EXECUTE FUNCTION webdock_rls.guard_git_build_attempt();

CREATE FUNCTION webdock_rls.cancel_git_build_attempt() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 IF NEW.status IN ('cancelled','failed','succeeded') OR NEW.generation<>OLD.generation THEN
  UPDATE webdock_auth.git_build_attempt SET cancel_requested=true WHERE build_id=NEW.id AND retired_at IS NULL AND NOT cancel_requested;
 END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_rls.cancel_git_build_attempt() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.cancel_git_build_attempt() FROM PUBLIC;
CREATE TRIGGER git_build_attempt_cancel AFTER UPDATE ON webdock_auth.git_build FOR EACH ROW EXECUTE FUNCTION webdock_rls.cancel_git_build_attempt();
