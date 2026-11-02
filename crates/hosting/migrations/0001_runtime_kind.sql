-- Offline additive migration. Existing k3s projects remain Kubernetes projects.
ALTER TABLE webdock_auth.hosting_cluster
 ADD COLUMN IF NOT EXISTS runtime_kind text NOT NULL DEFAULT 'kubernetes',
 ADD COLUMN IF NOT EXISTS runtime_installation text NOT NULL DEFAULT 'webdock';
DO $$ BEGIN
 IF NOT EXISTS(SELECT 1 FROM pg_constraint WHERE conrelid='webdock_auth.hosting_cluster'::regclass AND conname='hosting_cluster_runtime_kind_check') THEN
  ALTER TABLE webdock_auth.hosting_cluster ADD CONSTRAINT hosting_cluster_runtime_kind_check CHECK(runtime_kind IN ('kubernetes','docker'));
 END IF;
 IF NOT EXISTS(SELECT 1 FROM pg_constraint WHERE conrelid='webdock_auth.hosting_cluster'::regclass AND conname='hosting_cluster_runtime_installation_check') THEN
  ALTER TABLE webdock_auth.hosting_cluster ADD CONSTRAINT hosting_cluster_runtime_installation_check CHECK(runtime_installation ~ '^[a-z][a-z0-9-]{0,23}$');
 END IF;
END $$;
ALTER TABLE webdock_auth.hosting_project DROP CONSTRAINT IF EXISTS hosting_project_provider_check;
ALTER TABLE webdock_auth.hosting_project ADD CONSTRAINT hosting_project_provider_check CHECK(provider IN ('k3s','docker','vercel'));
ALTER TABLE webdock_auth.hosting_project DROP CONSTRAINT IF EXISTS hosting_project_check;
ALTER TABLE webdock_auth.hosting_project ADD CONSTRAINT hosting_project_check CHECK(
 (provider IN ('k3s','docker') AND cluster_id IS NOT NULL AND namespace IS NOT NULL)
 OR (provider='vercel' AND cluster_id IS NULL AND namespace IS NULL));
CREATE OR REPLACE FUNCTION webdock_rls.hosting_runtime_kind(cluster_id text) RETURNS text
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 IF webdock_rls.capacity_cluster_allowed(cluster_id) IS NOT TRUE THEN
  RAISE EXCEPTION 'Cluster authorization required' USING ERRCODE='42501';
 END IF;
 RETURN (SELECT runtime_kind FROM webdock_auth.hosting_cluster c WHERE c.id=hosting_runtime_kind.cluster_id);
END $$;
ALTER FUNCTION webdock_rls.hosting_runtime_kind(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.hosting_runtime_kind(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.hosting_runtime_kind(text) TO webdock_auth_runtime,webdock_worker_runtime;
-- capacity_cluster_allowed/capacity_demand aggregate by ownership and cluster ID,
-- not provider name. Their reviewed cross-tenant Mail/DB accounting stays intact.

-- Reconciliation can accept fresh authority without overwriting the original actor/idempotency scope.
ALTER TABLE webdock_auth.hosting_operation ADD COLUMN IF NOT EXISTS authority_subject text, ADD COLUMN IF NOT EXISTS authority_session text;

