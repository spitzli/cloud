-- Private scheduler retry state. Existing builds become immediately eligible; admission remains disabled.
ALTER TABLE webdock_auth.git_build
 ADD COLUMN retry_count integer NOT NULL DEFAULT 0 CHECK(retry_count BETWEEN 0 AND 2),
 ADD COLUMN retry_generation integer,
 ADD COLUMN next_eligible_at timestamptz NOT NULL DEFAULT now(),
 ADD COLUMN last_infrastructure_failure text CHECK(last_infrastructure_failure IN ('KUBERNETES_API_UNAVAILABLE','KUBERNETES_NODE_UNAVAILABLE')),
 ADD CONSTRAINT git_build_retry_metadata CHECK (
  (retry_count=0 AND retry_generation IS NULL AND last_infrastructure_failure IS NULL)
  OR (execution_backend='kubernetes' AND retry_count>0 AND retry_generation>0 AND retry_generation<=generation AND last_infrastructure_failure IS NOT NULL));
CREATE INDEX git_build_attempt_fair_tenant ON webdock_auth.git_build_attempt(customer_id,created_at DESC);
CREATE INDEX git_build_attempt_fair_project ON webdock_auth.git_build_attempt(project_id,created_at DESC);
CREATE INDEX git_build_kubernetes_eligible ON webdock_auth.git_build(next_eligible_at,created_at,id) WHERE status='queued' AND execution_backend='kubernetes';

CREATE FUNCTION webdock_rls.guard_git_build_infrastructure_retry() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 IF TG_OP='INSERT' THEN
  IF NEW.retry_count<>0 OR NEW.retry_generation IS NOT NULL OR NEW.last_infrastructure_failure IS NOT NULL THEN
   RAISE EXCEPTION 'retry requires a retired attempt' USING ERRCODE='23514';
  END IF;
 ELSIF ROW(NEW.retry_count,NEW.retry_generation,NEW.next_eligible_at,NEW.last_infrastructure_failure)
   IS DISTINCT FROM ROW(OLD.retry_count,OLD.retry_generation,OLD.next_eligible_at,OLD.last_infrastructure_failure)
   OR (NEW.execution_backend='kubernetes' AND OLD.status='failed' AND NEW.status='queued') THEN
  IF NEW.execution_backend<>'kubernetes' OR OLD.status<>'failed' OR NEW.status<>'queued'
   OR NEW.retry_count<>OLD.retry_count+1 OR NEW.retry_count>2
   OR NEW.retry_generation IS DISTINCT FROM OLD.generation OR NEW.generation<>OLD.generation
   OR NEW.next_eligible_at < now()+(CASE WHEN OLD.retry_count=0 THEN interval '30 seconds' ELSE interval '120 seconds' END)
   OR NEW.last_infrastructure_failure NOT IN ('KUBERNETES_API_UNAVAILABLE','KUBERNETES_NODE_UNAVAILABLE')
   OR (OLD.failure_code IS DISTINCT FROM 'KUBERNETES_JOB_RETIRED' AND OLD.failure_code IS DISTINCT FROM NEW.last_infrastructure_failure)
   OR ROW(NEW.id,NEW.customer_id,NEW.project_id,NEW.source_id,NEW.source_sha,NEW.source_revision,NEW.environment_revision,NEW.connection_generation,NEW.environment_identity)
      IS DISTINCT FROM ROW(OLD.id,OLD.customer_id,OLD.project_id,OLD.source_id,OLD.source_sha,OLD.source_revision,OLD.environment_revision,OLD.connection_generation,OLD.environment_identity)
   OR NOT EXISTS(SELECT 1 FROM webdock_auth.git_build_attempt a WHERE a.build_id=OLD.id AND a.generation=OLD.generation
      AND a.retired_at IS NOT NULL AND (a.storage_request IS NULL OR a.storage_retired_at IS NOT NULL)) THEN
   RAISE EXCEPTION 'retry requires a classified retired attempt and bounded backoff' USING ERRCODE='23514';
  END IF;
 END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_rls.guard_git_build_infrastructure_retry() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.guard_git_build_infrastructure_retry() FROM PUBLIC;
CREATE TRIGGER git_build_infrastructure_retry BEFORE INSERT OR UPDATE ON webdock_auth.git_build
 FOR EACH ROW EXECUTE FUNCTION webdock_rls.guard_git_build_infrastructure_retry();
