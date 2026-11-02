-- Opt-in quality workflow completion. Existing signed push sources keep their behavior.
ALTER TABLE webdock_auth.git_source ADD COLUMN webhook_trigger text NOT NULL DEFAULT 'push'
 CHECK(webhook_trigger IN ('push','workflow_run'));
ALTER TABLE webdock_auth.git_source ADD CONSTRAINT git_source_workflow_native CHECK (
 webhook_trigger<>'workflow_run' OR (execution_backend='kubernetes' AND build_provider='isolated'));
CREATE FUNCTION webdock_rls.guard_git_workflow_trigger() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 IF (NEW.webhook_trigger IS DISTINCT FROM OLD.webhook_trigger
  OR (NEW.webhook_trigger='workflow_run' OR OLD.webhook_trigger='workflow_run')
   AND NEW.workflow_path IS DISTINCT FROM OLD.workflow_path)
  AND NEW.revision<=OLD.revision THEN
  RAISE EXCEPTION 'workflow trigger requires new source revision' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_rls.guard_git_workflow_trigger() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.guard_git_workflow_trigger() FROM PUBLIC;
CREATE TRIGGER git_source_workflow_trigger BEFORE UPDATE OF webhook_trigger,workflow_path
 ON webdock_auth.git_source FOR EACH ROW EXECUTE FUNCTION webdock_rls.guard_git_workflow_trigger();
ALTER TABLE webdock_auth.git_build DROP CONSTRAINT git_build_trigger_kind_check;
ALTER TABLE webdock_auth.git_build ADD CONSTRAINT git_build_trigger_kind_check
 CHECK(trigger_kind IN ('manual','github-push','github-workflow'));

-- An older push-only ingester must not bypass an opted-in workflow gate after a binary rollback.
CREATE FUNCTION webdock_rls.guard_git_build_source_trigger() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog AS $$
DECLARE mode text; backend text;
BEGIN
 IF TG_OP='UPDATE' AND NEW.trigger_kind IS DISTINCT FROM OLD.trigger_kind THEN
  RAISE EXCEPTION 'build trigger is immutable' USING ERRCODE='23514';
 END IF;
 IF NEW.trigger_kind NOT IN ('github-push','github-workflow') THEN RETURN NEW; END IF;
 SELECT s.webhook_trigger,s.execution_backend INTO mode,backend FROM webdock_auth.git_source s WHERE s.id=NEW.source_id;
 IF (NEW.trigger_kind='github-push' AND mode='workflow_run')
  OR (NEW.trigger_kind='github-workflow' AND (mode IS DISTINCT FROM 'workflow_run' OR backend IS DISTINCT FROM 'kubernetes')) THEN
  RAISE EXCEPTION 'build source workflow gate changed' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_rls.guard_git_build_source_trigger() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.guard_git_build_source_trigger() FROM PUBLIC;
CREATE TRIGGER git_build_source_trigger BEFORE INSERT OR UPDATE OF trigger_kind
 ON webdock_auth.git_build FOR EACH ROW EXECUTE FUNCTION webdock_rls.guard_git_build_source_trigger();

-- Resolve-only source consent authority. No synthetic session and no archive capability.
ALTER TABLE webdock_auth.repository_lease DROP CONSTRAINT repository_lease_authority_kind_check;
ALTER TABLE webdock_auth.repository_lease ADD CONSTRAINT repository_lease_authority_kind_check
 CHECK(authority_kind IN ('session','build','release','plugin-job','source-trigger'));
ALTER TABLE webdock_auth.repository_lease ADD CONSTRAINT repository_source_trigger_bound CHECK (
 authority_kind<>'source-trigger' OR (action='resolve' AND authority_id IS NOT NULL AND authority_id=source_id
  AND authority_generation IS NOT NULL AND authority_generation>0 AND actor_session IS NULL AND actor_authority IS NULL
  AND archive_authority IS NULL AND worker_id IS NULL AND worker_generation IS NULL
  AND worker_credential_hash IS NULL));
ALTER FUNCTION webdock_rls.repository_domain_live(uuid) RENAME TO repository_domain_live_before_source_triggers;
REVOKE ALL ON FUNCTION webdock_rls.repository_domain_live_before_source_triggers(uuid) FROM PUBLIC,webdock_worker_runtime;
CREATE FUNCTION webdock_rls.repository_domain_live(lease_id uuid) RETURNS jsonb
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE l webdock_auth.repository_lease%ROWTYPE; s webdock_auth.git_source%ROWTYPE;
 c webdock_auth.git_connection%ROWTYPE; actor jsonb;
BEGIN
 IF NOT webdock_rls.worker(ARRAY['repository-admission']) THEN RETURN NULL; END IF;
 SELECT * INTO l FROM webdock_auth.repository_lease x WHERE x.id=lease_id AND x.state='active' AND x.expires_at>clock_timestamp();
 IF NOT FOUND THEN RETURN NULL; END IF;
 IF l.authority_kind<>'source-trigger' THEN RETURN webdock_rls.repository_domain_live_before_source_triggers(lease_id); END IF;
 SELECT * INTO s FROM webdock_auth.git_source x WHERE x.id=l.source_id AND x.project_id=l.project_id
  AND x.customer_id=l.customer_id AND x.enabled AND x.revision=l.source_revision
  AND x.execution_backend='kubernetes' AND x.build_provider='isolated' AND x.webhook_trigger='workflow_run'
  AND x.webhook_enabled AND x.webhook_revision=x.revision AND x.webhook_environment_revision=x.environment_revision
  AND x.webhook_subject=l.actor_subject AND l.authority_id=x.id AND l.authority_generation=x.environment_revision;
 IF NOT FOUND OR l.action<>'resolve' OR l.input_json::jsonb->>'reference' IS DISTINCT FROM s.branch THEN RETURN NULL; END IF;
 SELECT * INTO c FROM webdock_auth.git_connection x WHERE x.id=s.connection_id AND x.customer_id=s.customer_id
  AND x.state='active' AND x.generation=l.connection_generation AND x.generation=s.webhook_connection_generation
  AND x.repository_plugin_id=l.plugin_id;
 IF NOT FOUND OR NOT EXISTS(SELECT 1 FROM webdock_auth.git_repository r WHERE r.connection_id=c.id
   AND r.repository_id=s.repository_id AND r.customer_id=s.customer_id)
  OR l.input_json::jsonb->>'installationID' IS DISTINCT FROM c.installation_id
  OR l.input_json::jsonb->>'repositoryID' IS DISTINCT FROM s.repository_id THEN RETURN NULL; END IF;
 actor:=webdock_rls.git_actor_authority_internal(s.customer_id,l.actor_subject,NULL);
 IF actor IS NULL OR NOT EXISTS(SELECT 1 FROM webdock_admin.projects p JOIN webdock_admin.customers u ON u.id=p.customer_id
  JOIN webdock_auth.hosting_project h ON h.project_id=p.id AND h.customer_id=u.id
  WHERE p.id=s.project_id AND p.customer_id=s.customer_id AND p.status='active' AND u.status='active'
   AND (h.mode='selfservice' OR coalesce((actor->>'operator')::boolean,false))
   AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=p.id AND d.status<>'deleted'))
  OR NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_installation p WHERE p.id='hosting' AND p.state='enabled')
  OR NOT EXISTS(SELECT 1 FROM webdock_auth.git_build_trusted_project t JOIN webdock_auth.git_build_admission a ON a.singleton
   WHERE t.project_id=s.project_id AND t.customer_id=s.customer_id AND t.enabled AND a.enabled) THEN RETURN NULL; END IF;
 RETURN actor;
END $$;
ALTER FUNCTION webdock_rls.repository_domain_live(uuid) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.repository_domain_live(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.repository_domain_live(uuid) TO webdock_worker_runtime;
