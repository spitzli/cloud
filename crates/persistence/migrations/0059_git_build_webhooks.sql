-- Explicit build-trigger consent is independent of deployment/auto-publication approval.
ALTER TABLE webdock_auth.git_source ADD COLUMN webhook_enabled boolean NOT NULL DEFAULT false;
ALTER TABLE webdock_auth.git_source ADD COLUMN webhook_subject text;
ALTER TABLE webdock_auth.git_source ADD COLUMN webhook_revision integer;
ALTER TABLE webdock_auth.git_source ADD COLUMN webhook_connection_generation integer;
ALTER TABLE webdock_auth.git_source ADD COLUMN webhook_environment_revision integer;
ALTER TABLE webdock_auth.git_source ADD CONSTRAINT git_source_webhook_consent CHECK (
 NOT webhook_enabled OR (webhook_subject IS NOT NULL AND webhook_revision IS NOT NULL AND webhook_revision>0
  AND webhook_connection_generation IS NOT NULL AND webhook_connection_generation>0
  AND webhook_environment_revision IS NOT NULL AND webhook_environment_revision>0));
ALTER TABLE webdock_auth.git_build ADD COLUMN trigger_kind text NOT NULL DEFAULT 'manual'
 CHECK(trigger_kind IN ('manual','github-push'));

CREATE TABLE webdock_auth.git_webhook_delivery (
 provider text NOT NULL CHECK(provider='github'),
 delivery_id uuid NOT NULL,
 repository_plugin_id text NOT NULL,
 body_sha256 text NOT NULL CHECK(body_sha256 ~ '^[a-f0-9]{64}$'),
 event text NOT NULL CHECK(event ~ '^[a-z_]{1,64}$'),
 build_ids jsonb NOT NULL CHECK(jsonb_typeof(build_ids)='array' AND octet_length(build_ids::text)<=65536),
 created_at timestamptz NOT NULL DEFAULT now(),
 PRIMARY KEY(provider,delivery_id)
);
ALTER TABLE webdock_auth.git_webhook_delivery OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.git_webhook_delivery ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.git_webhook_delivery FORCE ROW LEVEL SECURITY;
CREATE POLICY platform_owner ON webdock_auth.git_webhook_delivery TO webdock_platform_owner USING(true) WITH CHECK(true);
CREATE POLICY worker_read ON webdock_auth.git_webhook_delivery FOR SELECT TO webdock_worker_runtime
 USING(webdock_rls.worker(ARRAY['git-worker']));
CREATE POLICY worker_insert ON webdock_auth.git_webhook_delivery FOR INSERT TO webdock_worker_runtime
 WITH CHECK(webdock_rls.worker(ARRAY['git-worker']));
GRANT SELECT,INSERT ON webdock_auth.git_webhook_delivery TO webdock_worker_runtime;

CREATE FUNCTION webdock_rls.git_source_build_trigger_authority(source_id text,source_revision integer)
RETURNS jsonb LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE s webdock_auth.git_source%ROWTYPE; actor jsonb;
BEGIN
 IF NOT webdock_rls.worker(ARRAY['git-worker']) THEN RETURN NULL; END IF;
 SELECT * INTO s FROM webdock_auth.git_source x WHERE x.id=source_id AND x.revision=source_revision
  AND x.enabled AND x.webhook_enabled AND x.webhook_revision=x.revision
  AND x.webhook_environment_revision=x.environment_revision AND x.build_provider='isolated'
  AND x.recipe IN ('dockerfile','dockerfile-frontend') FOR SHARE;
 IF NOT FOUND OR NOT EXISTS(SELECT 1 FROM webdock_auth.git_connection c
  JOIN webdock_auth.git_repository r ON r.connection_id=c.id AND r.repository_id=s.repository_id AND r.customer_id=s.customer_id
  WHERE c.id=s.connection_id AND c.customer_id=s.customer_id AND c.state='active'
   AND c.generation=s.webhook_connection_generation AND c.repository_plugin_id IS NOT NULL) THEN RETURN NULL; END IF;
 actor:=webdock_rls.git_actor_authority_internal(s.customer_id,s.webhook_subject,NULL);
 IF actor IS NULL OR NOT EXISTS(SELECT 1 FROM webdock_admin.projects p
  JOIN webdock_admin.customers c ON c.id=p.customer_id JOIN webdock_auth.hosting_project h ON h.project_id=p.id AND h.customer_id=c.id
  WHERE p.id=s.project_id AND p.customer_id=s.customer_id AND p.status='active' AND c.status='active'
   AND (h.mode='selfservice' OR coalesce((actor->>'operator')::boolean,false))
   AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=p.id AND d.status<>'deleted')) THEN RETURN NULL; END IF;
 RETURN actor||jsonb_build_object('customerID',s.customer_id,'projectID',s.project_id,'sourceID',s.id);
END $$;
ALTER FUNCTION webdock_rls.git_source_build_trigger_authority(text,integer) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.git_source_build_trigger_authority(text,integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.git_source_build_trigger_authority(text,integer) TO webdock_worker_runtime;
