CREATE TABLE webdock_auth.api_job (
 id text PRIMARY KEY DEFAULT webdock_auth.next_snowflake(),
 name text NOT NULL CHECK (name IN ('storage.refresh','mail.dispatch','mail.events','mail.webhooks','mail.maintenance')),
 slot timestamptz NOT NULL,
 status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','running','succeeded','failed')),
 attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
 available_at timestamptz NOT NULL DEFAULT now(),
 claim_token uuid,
 lease_until timestamptz,
 finished_at timestamptz,
 UNIQUE(name,slot)
);
CREATE INDEX api_job_due ON webdock_auth.api_job(available_at,slot) WHERE status IN ('pending','running');
ALTER TABLE webdock_auth.api_job OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.api_job ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.api_job FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_auth.api_job FROM PUBLIC;
GRANT SELECT,INSERT,UPDATE ON webdock_auth.api_job TO webdock_worker_runtime;
CREATE POLICY api_job_worker ON webdock_auth.api_job TO webdock_worker_runtime
 USING (webdock_rls.worker(ARRAY['api-scheduler'])) WITH CHECK (webdock_rls.worker(ARRAY['api-scheduler']));
GRANT SELECT ON webdock_auth.api_job TO webdock_auth_runtime;
CREATE POLICY api_job_operator ON webdock_auth.api_job FOR SELECT TO webdock_auth_runtime USING (webdock_rls.operator());
