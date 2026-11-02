-- Complete immutable Kubernetes budgets, including cache, survive worker upgrades.
-- Existing private attempts retain the exact former executor defaults.
ALTER TABLE webdock_auth.git_build_attempt ADD COLUMN kubernetes_limits jsonb;
ALTER TABLE webdock_auth.git_build_attempt DISABLE TRIGGER git_build_attempt_guard;
UPDATE webdock_auth.git_build_attempt SET kubernetes_limits=jsonb_build_object(
 'cpu_millis',(execution_request#>>'{limits,cpu}')::integer*1000,
 'memory_mib',(execution_request#>>'{limits,memoryMiB}')::integer,
 'workspace_mib',((execution_request#>>'{limits,diskBytes}')::bigint+1048575)/1048576,
 'cache_mib',16384,'deadline_seconds',(execution_request#>>'{limits,durationSeconds}')::integer);
ALTER TABLE webdock_auth.git_build_attempt ENABLE TRIGGER git_build_attempt_guard;
ALTER TABLE webdock_auth.git_build_attempt ALTER COLUMN kubernetes_limits SET NOT NULL;
ALTER TABLE webdock_auth.git_build_attempt ADD CONSTRAINT git_build_attempt_resource_budget CHECK(
 jsonb_typeof(kubernetes_limits)='object' AND octet_length(kubernetes_limits::text)<=2048
 AND kubernetes_limits ?& ARRAY['cpu_millis','memory_mib','workspace_mib','cache_mib','deadline_seconds']
 AND kubernetes_limits-ARRAY['cpu_millis','memory_mib','workspace_mib','cache_mib','deadline_seconds']='{}'::jsonb
 AND jsonb_typeof(kubernetes_limits->'cpu_millis')='number'
 AND jsonb_typeof(kubernetes_limits->'memory_mib')='number'
 AND jsonb_typeof(kubernetes_limits->'workspace_mib')='number'
 AND jsonb_typeof(kubernetes_limits->'cache_mib')='number'
 AND jsonb_typeof(kubernetes_limits->'deadline_seconds')='number'
 AND (kubernetes_limits->>'cpu_millis')::numeric BETWEEN 250 AND 8000
 AND (kubernetes_limits->>'memory_mib')::numeric BETWEEN 512 AND 16384
 AND (kubernetes_limits->>'workspace_mib')::numeric BETWEEN 512 AND 32768
 AND (kubernetes_limits->>'cache_mib')::numeric BETWEEN 64 AND 16384
 AND (kubernetes_limits->>'deadline_seconds')::numeric BETWEEN 30 AND 1800
 AND (kubernetes_limits->>'cpu_millis') ~ '^[0-9]{1,5}$'
 AND (kubernetes_limits->>'memory_mib') ~ '^[0-9]{1,5}$'
 AND (kubernetes_limits->>'workspace_mib') ~ '^[0-9]{1,5}$'
 AND (kubernetes_limits->>'cache_mib') ~ '^[0-9]{1,5}$'
 AND (kubernetes_limits->>'deadline_seconds') ~ '^[0-9]{1,4}$'
);
CREATE FUNCTION webdock_rls.guard_git_build_resource_budget() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 IF TG_OP='INSERT' THEN
  -- Compatibility with an older private controller: snapshot its known defaults
  -- once on INSERT. Reconciliation never recomputes from a newer worker's defaults.
  IF NEW.kubernetes_limits IS NULL THEN
   NEW.kubernetes_limits:=jsonb_build_object(
    'cpu_millis',(NEW.execution_request#>>'{limits,cpu}')::integer*1000,
    'memory_mib',(NEW.execution_request#>>'{limits,memoryMiB}')::integer,
    'workspace_mib',((NEW.execution_request#>>'{limits,diskBytes}')::bigint+1048575)/1048576,
    'cache_mib',16384,'deadline_seconds',(NEW.execution_request#>>'{limits,durationSeconds}')::integer);
  END IF;
 ELSIF NEW.kubernetes_limits IS DISTINCT FROM OLD.kubernetes_limits THEN
  RAISE EXCEPTION 'attempt resource budget is immutable' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_rls.guard_git_build_resource_budget() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.guard_git_build_resource_budget() FROM PUBLIC;
CREATE TRIGGER git_build_attempt_budget BEFORE INSERT OR UPDATE ON webdock_auth.git_build_attempt
 FOR EACH ROW EXECUTE FUNCTION webdock_rls.guard_git_build_resource_budget();
