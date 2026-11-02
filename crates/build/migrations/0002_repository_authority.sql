-- Native0013: distinct repository service leases. Ordinary plugin session Invoke is unchanged.
ALTER TABLE webdock_auth.git_connection ADD COLUMN IF NOT EXISTS repository_plugin_id text;
CREATE TABLE IF NOT EXISTS webdock_auth.repository_lease (
 id uuid PRIMARY KEY,
 plugin_id text NOT NULL,
 customer_id text NOT NULL,
 project_id text NOT NULL,
 source_id text NOT NULL,
 generation bigint NOT NULL CHECK(generation>0),
 source_revision integer NOT NULL CHECK(source_revision>0),
 connection_generation integer NOT NULL CHECK(connection_generation>0),
 grant_revision bigint NOT NULL CHECK(grant_revision>=0),
 configuration_revision bigint NOT NULL CHECK(configuration_revision>=0),
 action text NOT NULL CHECK(action IN ('resolve','archive','check')),
 descriptor_sha256 text NOT NULL CHECK(descriptor_sha256~'^[a-f0-9]{64}$'),
 binding_sha256 text NOT NULL CHECK(binding_sha256~'^[a-f0-9]{64}$'),
 request_sha256 text NOT NULL CHECK(request_sha256~'^[a-f0-9]{64}$'),
 configuration_sha256 text NOT NULL CHECK(configuration_sha256~'^[a-f0-9]{64}$'),
 input_json text NOT NULL CHECK(octet_length(input_json)<=32768),
 configuration_json text NOT NULL CHECK(octet_length(configuration_json)<=65536),
 authority_kind text NOT NULL CHECK(authority_kind IN ('session','build','release')),
 actor_subject text NOT NULL,
 actor_session text,
 authority_id text,
 authority_generation integer,
 worker_id text,
 worker_generation integer,
 worker_credential_hash text,
 issued_at timestamptz NOT NULL,
 expires_at timestamptz NOT NULL CHECK(expires_at>issued_at AND expires_at<=issued_at+interval '60 seconds'),
 state text NOT NULL DEFAULT 'active' CHECK(state IN ('active','complete','revoked'))
);
-- Ephemeral references intentionally have no row-locking foreign keys: interactive source
-- resolution already holds the source in its caller transaction. Every use checks live ownership.
ALTER TABLE webdock_auth.repository_lease OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.repository_lease ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.repository_lease FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_auth.repository_lease FROM PUBLIC,webdock_auth_runtime,webdock_auth_bootstrap,webdock_registry_runtime,webdock_worker_runtime;
GRANT SELECT,INSERT,UPDATE,DELETE ON webdock_auth.repository_lease TO webdock_worker_runtime;
DROP POLICY IF EXISTS platform_owner ON webdock_auth.repository_lease;
CREATE POLICY platform_owner ON webdock_auth.repository_lease TO webdock_platform_owner USING(true) WITH CHECK(true);
DROP POLICY IF EXISTS repository_admission ON webdock_auth.repository_lease;
CREATE POLICY repository_admission ON webdock_auth.repository_lease TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['repository-admission'])) WITH CHECK(webdock_rls.worker(ARRAY['repository-admission']));
CREATE INDEX IF NOT EXISTS repository_lease_expiry ON webdock_auth.repository_lease(expires_at);

CREATE OR REPLACE FUNCTION webdock_rls.repository_domain_live(lease_id uuid) RETURNS jsonb
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE l webdock_auth.repository_lease%ROWTYPE; s webdock_auth.git_source%ROWTYPE;
 c webdock_auth.git_connection%ROWTYPE; b webdock_auth.git_build%ROWTYPE;
 r webdock_auth.git_release%ROWTYPE; actor jsonb;
BEGIN
 IF NOT webdock_rls.worker(ARRAY['repository-admission']) THEN RETURN NULL; END IF;
 SELECT * INTO l FROM webdock_auth.repository_lease x WHERE x.id=lease_id AND x.state='active' AND x.expires_at>now();
 IF NOT FOUND THEN RETURN NULL; END IF;
 SELECT * INTO s FROM webdock_auth.git_source x WHERE x.id=l.source_id AND x.project_id=l.project_id AND x.customer_id=l.customer_id AND x.enabled AND x.revision=l.source_revision;
 IF NOT FOUND THEN RETURN NULL; END IF;
 SELECT * INTO c FROM webdock_auth.git_connection x WHERE x.id=s.connection_id AND x.customer_id=s.customer_id AND x.state='active' AND x.generation=l.connection_generation AND x.repository_plugin_id=l.plugin_id;
 IF NOT FOUND OR NOT EXISTS(SELECT 1 FROM webdock_auth.git_repository q WHERE q.connection_id=c.id AND q.repository_id=s.repository_id AND q.customer_id=s.customer_id) OR (l.input_json::jsonb->>'installationID') IS DISTINCT FROM c.installation_id OR (l.input_json::jsonb->>'repositoryID') IS DISTINCT FROM s.repository_id THEN RETURN NULL; END IF;
 IF NOT EXISTS(SELECT 1 FROM webdock_admin.projects p JOIN webdock_admin.customers u ON u.id=p.customer_id WHERE p.id=s.project_id AND p.customer_id=s.customer_id AND p.status='active' AND u.status='active' AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=p.id AND d.status<>'deleted')) OR NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_installation p WHERE p.id='hosting' AND p.state IN ('enabled','draining')) THEN RETURN NULL; END IF;
 IF l.authority_kind='session' THEN
  IF l.actor_session IS NULL OR l.action<>'resolve' OR NOT ((l.input_json::jsonb->>'reference')=s.branch OR (l.input_json::jsonb->>'reference')~'^[a-f0-9]{40}$') THEN RETURN NULL; END IF;
  actor:=webdock_rls.git_actor_authority_internal(l.customer_id,l.actor_subject,l.actor_session);
 ELSE
  IF NOT EXISTS(SELECT 1 FROM webdock_auth.git_worker w WHERE w.id=l.worker_id AND w.enabled AND w.generation=l.worker_generation AND w.credential_hash=l.worker_credential_hash) THEN RETURN NULL; END IF;
  IF l.authority_kind='build' THEN
   SELECT * INTO b FROM webdock_auth.git_build x WHERE x.id=l.authority_id AND x.source_id=s.id AND x.source_revision=s.revision AND x.connection_generation=c.generation AND x.environment_revision=s.environment_revision AND x.worker_id=l.worker_id AND x.worker_generation=l.worker_generation AND x.generation=l.authority_generation AND x.status='running' AND x.lease_until>now() AND x.subject=l.actor_subject;
   IF NOT FOUND OR l.action<>'archive' OR (l.input_json::jsonb->>'commit') IS DISTINCT FROM b.source_sha THEN RETURN NULL; END IF;
   actor:=webdock_rls.git_actor_authority_internal(l.customer_id,b.subject,NULL);
  ELSIF l.authority_kind='release' THEN
   SELECT * INTO r FROM webdock_auth.git_release x WHERE x.id=l.authority_id AND x.source_id=s.id AND x.source_revision=s.revision AND x.connection_generation=c.generation AND x.environment_revision=s.environment_revision AND x.environment_identity IS NOT DISTINCT FROM s.environment_identity AND x.id=s.desired_release_id AND x.worker_id=l.worker_id AND x.worker_generation=l.worker_generation AND x.generation=l.authority_generation AND x.status='deploying' AND x.lease_until>now() AND x.approved_by=l.actor_subject;
   IF NOT FOUND OR l.action<>'resolve' OR NOT EXISTS(SELECT 1 FROM webdock_auth.git_artifact a WHERE a.id=r.artifact_id AND a.retained AND a.customer_id=r.customer_id AND a.project_id=r.project_id) THEN RETURN NULL; END IF;
   IF r.approved_source='git-policy' AND (l.input_json::jsonb->>'reference')=s.branch AND s.auto_publish AND s.policy_revision=s.revision AND s.policy_subject=r.approved_by THEN actor:=webdock_rls.git_actor_authority_internal(l.customer_id,r.approved_by,NULL);
   ELSIF r.approved_source IN ('studio','oauth') AND (l.input_json::jsonb->>'reference')=(SELECT x.source_sha FROM webdock_auth.git_artifact a JOIN webdock_auth.git_build x ON x.id=a.build_id WHERE a.id=r.artifact_id) AND r.approved_session_id IS NOT NULL AND r.approved_session_id<>'' AND (r.approved_source<>'oauth' OR coalesce(r.approved_scopes @> '["hosting:write"]'::jsonb,false)) THEN actor:=webdock_rls.git_actor_authority_internal(l.customer_id,r.approved_by,r.approved_session_id);
   ELSE RETURN NULL; END IF;
  ELSE RETURN NULL; END IF;
 END IF;
 IF actor IS NULL OR NOT EXISTS(SELECT 1 FROM webdock_auth.hosting_project h WHERE h.project_id=l.project_id AND h.customer_id=l.customer_id AND (h.mode='selfservice' OR coalesce((actor->>'operator')::boolean,false))) THEN RETURN NULL; END IF;
 RETURN actor;
END $$;
ALTER FUNCTION webdock_rls.repository_domain_live(uuid) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.repository_domain_live(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.repository_domain_live(uuid) TO webdock_worker_runtime;

-- Rebinding the independent provider is a credential-scope change, even when a caller forgets
-- to advance the generation. Existing jobs and repository leases must become unusable.
CREATE OR REPLACE FUNCTION webdock_auth.repository_provider_generation() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 IF NEW.repository_plugin_id IS DISTINCT FROM OLD.repository_plugin_id THEN
  NEW.generation:=greatest(NEW.generation,OLD.generation+1);
 END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_auth.repository_provider_generation() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.repository_provider_generation() FROM PUBLIC;
DROP TRIGGER IF EXISTS repository_provider_generation ON webdock_auth.git_connection;
CREATE TRIGGER repository_provider_generation BEFORE UPDATE OF repository_plugin_id
ON webdock_auth.git_connection FOR EACH ROW EXECUTE FUNCTION webdock_auth.repository_provider_generation();
