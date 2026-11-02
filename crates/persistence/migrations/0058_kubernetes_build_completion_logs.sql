-- Versioned Kubernetes completion: OCI retention is not registry publication success.
ALTER TABLE webdock_auth.git_build DROP CONSTRAINT git_build_status_check;
ALTER TABLE webdock_auth.git_build ADD CONSTRAINT git_build_status_check CHECK (
 status IN ('queued','running','artifact-ready','succeeded','failed','cancelled')
 AND (status<>'artifact-ready' OR execution_backend='kubernetes')
);
ALTER TABLE webdock_auth.git_build ADD COLUMN phase text NOT NULL DEFAULT 'queued';
ALTER TABLE webdock_auth.git_build ADD COLUMN registry_verified_at timestamptz;
ALTER TABLE webdock_auth.git_build ADD COLUMN retained_log_bytes integer NOT NULL DEFAULT 0 CHECK (retained_log_bytes BETWEEN 0 AND 1048576);
ALTER TABLE webdock_auth.git_build ADD CONSTRAINT git_build_phase_check CHECK (
 phase IN ('queued','provisioning','cloning','preparing','building','pushing','awaiting-approval','succeeded','failed','cancelled')
);
UPDATE webdock_auth.git_build SET phase=CASE WHEN status='running' THEN 'building' ELSE status END;

CREATE TABLE webdock_auth.git_build_log_entry (
 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
 build_id varchar NOT NULL REFERENCES webdock_auth.git_build(id),
 customer_id varchar NOT NULL,
 project_id varchar NOT NULL,
 generation integer NOT NULL CHECK (generation>0),
 entry_key text NOT NULL CHECK (entry_key ~ '^[A-Za-z0-9_-]{16,128}$'),
 phase text NOT NULL CHECK (phase IN ('provisioning','cloning','preparing','building','pushing')),
 text text NOT NULL CHECK (octet_length(text)<=8192),
 receipt text NOT NULL CHECK (receipt ~ '^[a-f0-9]{64}$'),
 created_at timestamptz NOT NULL DEFAULT now(),
 UNIQUE(build_id,generation,entry_key)
);
ALTER TABLE webdock_auth.git_build_log_entry OWNER TO webdock_platform_owner;
CREATE INDEX git_build_log_entry_cursor ON webdock_auth.git_build_log_entry(build_id,id);
ALTER TABLE webdock_auth.git_build_log_entry ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.git_build_log_entry FORCE ROW LEVEL SECURITY;
CREATE POLICY platform_owner ON webdock_auth.git_build_log_entry TO webdock_platform_owner USING(true) WITH CHECK(true);
CREATE POLICY tenant_read ON webdock_auth.git_build_log_entry FOR SELECT TO webdock_auth_runtime USING(webdock_rls.customer_allowed(customer_id));
CREATE POLICY worker_read ON webdock_auth.git_build_log_entry FOR SELECT TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['git-worker']));
CREATE POLICY worker_insert ON webdock_auth.git_build_log_entry FOR INSERT TO webdock_worker_runtime WITH CHECK(webdock_rls.worker(ARRAY['git-worker']));
GRANT SELECT ON webdock_auth.git_build_log_entry TO webdock_auth_runtime;
GRANT SELECT,INSERT ON webdock_auth.git_build_log_entry TO webdock_worker_runtime;
GRANT USAGE ON SEQUENCE webdock_auth.git_build_log_entry_id_seq TO webdock_worker_runtime;

CREATE FUNCTION webdock_rls.guard_git_build_log_scope() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM webdock_auth.git_build b WHERE b.id=NEW.build_id
   AND b.customer_id=NEW.customer_id AND b.project_id=NEW.project_id AND b.generation=NEW.generation
   AND b.status='running' AND b.lease_until>now() AND b.execution_backend='kubernetes') THEN
  RAISE EXCEPTION 'build log scope changed' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_rls.guard_git_build_log_scope() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.guard_git_build_log_scope() FROM PUBLIC;
CREATE TRIGGER git_build_log_scope BEFORE INSERT ON webdock_auth.git_build_log_entry FOR EACH ROW EXECUTE FUNCTION webdock_rls.guard_git_build_log_scope();
