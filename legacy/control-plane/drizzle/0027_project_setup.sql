
CREATE TABLE IF NOT EXISTS webdock_auth.hosting_setup (
 id varchar PRIMARY KEY DEFAULT webdock_auth.next_snowflake(),
 customer_id varchar NOT NULL REFERENCES webdock_admin.customers(id),
 project_id varchar NOT NULL REFERENCES webdock_admin.projects(id),
 subject text NOT NULL,actor jsonb,configuration jsonb NOT NULL,environment_encrypted text,
 revision integer NOT NULL DEFAULT 1,state text NOT NULL DEFAULT 'draft' CHECK(state IN ('draft','awaiting-confirmation','running','blocked','failed','ready')),
 preview jsonb,confirmed_preview_id text,resources jsonb NOT NULL DEFAULT '{}',steps jsonb NOT NULL DEFAULT '[]',reason text,
 generation integer NOT NULL DEFAULT 0,lease_until timestamptz,next_attempt_at timestamptz NOT NULL DEFAULT now(),
 created_at timestamptz NOT NULL DEFAULT now(),updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS hosting_setup_due ON webdock_auth.hosting_setup(next_attempt_at) WHERE state='running';

ALTER TABLE webdock_auth.hosting_setup OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.hosting_setup ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.hosting_setup FORCE ROW LEVEL SECURITY;
GRANT SELECT,INSERT,UPDATE ON webdock_auth.hosting_setup TO webdock_auth_runtime;
CREATE POLICY setup_customer ON webdock_auth.hosting_setup TO webdock_auth_runtime USING(webdock_rls.customer_allowed(customer_id::text)) WITH CHECK(webdock_rls.customer_allowed(customer_id::text,true));
CREATE POLICY setup_owner ON webdock_auth.hosting_setup TO webdock_platform_owner USING(true) WITH CHECK(true);
ALTER TABLE webdock_auth.api_job DROP CONSTRAINT api_job_name_check;
ALTER TABLE webdock_auth.api_job ADD CONSTRAINT api_job_name_check CHECK(name IN ('storage.refresh','mail.dispatch','mail.events','mail.webhooks','mail.maintenance','hosting.setup'));
