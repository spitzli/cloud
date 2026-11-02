-- Named environments are deployment scope, not the existing configuration identity/hash.
CREATE TABLE webdock_auth.git_environment (
 project_id varchar NOT NULL REFERENCES webdock_admin.projects(id) ON DELETE CASCADE,
 name text NOT NULL CHECK(name ~ '^[a-z0-9][a-z0-9-]{0,62}$'),
 customer_id varchar NOT NULL REFERENCES webdock_admin.customers(id),
 type text NOT NULL CHECK(type IN ('production','preview')),
 expires_at timestamptz,
 created_at timestamptz NOT NULL DEFAULT now(),
 PRIMARY KEY(project_id,name),
 CHECK(type<>'production' OR (name='production' AND expires_at IS NULL))
);
ALTER TABLE webdock_auth.git_environment OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.git_environment ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.git_environment FORCE ROW LEVEL SECURITY;
CREATE POLICY platform_owner ON webdock_auth.git_environment TO webdock_platform_owner USING(true) WITH CHECK(true);
CREATE POLICY tenant_read ON webdock_auth.git_environment FOR SELECT TO webdock_auth_runtime USING(webdock_rls.customer_allowed(customer_id));
CREATE POLICY worker_read ON webdock_auth.git_environment FOR SELECT TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['git-worker']));
GRANT SELECT ON webdock_auth.git_environment TO webdock_auth_runtime,webdock_worker_runtime;
INSERT INTO webdock_auth.git_environment(project_id,name,customer_id,type)
 SELECT DISTINCT project_id,'production',customer_id,'production' FROM webdock_auth.git_source;
ALTER TABLE webdock_auth.git_source ADD COLUMN environment_name text NOT NULL DEFAULT 'production' CHECK(environment_name='production');
ALTER TABLE webdock_auth.git_build ADD COLUMN environment_name text NOT NULL DEFAULT 'production' CHECK(environment_name='production');
ALTER TABLE webdock_auth.git_release ADD COLUMN environment_name text NOT NULL DEFAULT 'production' CHECK(environment_name='production');
ALTER TABLE webdock_auth.git_source ADD FOREIGN KEY(project_id,environment_name) REFERENCES webdock_auth.git_environment(project_id,name);
ALTER TABLE webdock_auth.git_build ADD FOREIGN KEY(project_id,environment_name) REFERENCES webdock_auth.git_environment(project_id,name);
ALTER TABLE webdock_auth.git_release ADD FOREIGN KEY(project_id,environment_name) REFERENCES webdock_auth.git_environment(project_id,name);
CREATE FUNCTION webdock_rls.ensure_git_production_environment() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 IF NEW.environment_name<>'production' OR NOT EXISTS(SELECT 1 FROM webdock_admin.projects p WHERE p.id=NEW.project_id AND p.customer_id=NEW.customer_id) THEN
  RAISE EXCEPTION 'environment project scope mismatch' USING ERRCODE='23514';
 END IF;
 INSERT INTO webdock_auth.git_environment(project_id,name,customer_id,type) VALUES(NEW.project_id,'production',NEW.customer_id,'production') ON CONFLICT(project_id,name) DO NOTHING;
 IF NOT EXISTS(SELECT 1 FROM webdock_auth.git_environment e WHERE e.project_id=NEW.project_id AND e.name='production' AND e.customer_id=NEW.customer_id AND e.type='production') THEN
  RAISE EXCEPTION 'environment ownership mismatch' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_rls.ensure_git_production_environment() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.ensure_git_production_environment() FROM PUBLIC;
CREATE TRIGGER git_source_environment BEFORE INSERT OR UPDATE OF project_id,customer_id,environment_name ON webdock_auth.git_source FOR EACH ROW EXECUTE FUNCTION webdock_rls.ensure_git_production_environment();
