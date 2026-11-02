
CREATE UNIQUE INDEX IF NOT EXISTS hosting_app_project_identity ON webdock_auth.hosting_app(id,project_id);
CREATE TABLE IF NOT EXISTS webdock_auth.hosting_service_binding (
 id varchar PRIMARY KEY DEFAULT webdock_auth.next_snowflake(),
 customer_id varchar NOT NULL,project_id varchar NOT NULL,app_id varchar NOT NULL,
 service jsonb NOT NULL,variable text NOT NULL,
 revision integer NOT NULL DEFAULT 1,observed_revision integer NOT NULL DEFAULT 0,
 credential_revision integer,secret_encrypted text,
 state text NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','connected','blocked','disconnecting','disconnected')),
 reason text,operation_id varchar REFERENCES webdock_auth.hosting_operation(id),
 created_at timestamptz NOT NULL DEFAULT now(),
 FOREIGN KEY(project_id,customer_id) REFERENCES webdock_auth.hosting_project(project_id,customer_id),
 FOREIGN KEY(app_id,project_id) REFERENCES webdock_auth.hosting_app(id,project_id)
);
CREATE UNIQUE INDEX IF NOT EXISTS hosting_binding_variable ON webdock_auth.hosting_service_binding(app_id,variable) WHERE state<>'disconnected';

ALTER TABLE webdock_auth.hosting_service_binding OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.hosting_service_binding ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.hosting_service_binding FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_auth.hosting_service_binding FROM PUBLIC;
GRANT SELECT,INSERT,UPDATE ON webdock_auth.hosting_service_binding TO webdock_auth_runtime;
CREATE POLICY binding_customer ON webdock_auth.hosting_service_binding TO webdock_auth_runtime USING(webdock_rls.customer_allowed(customer_id::text)) WITH CHECK(webdock_rls.customer_allowed(customer_id::text,true));
GRANT SELECT,UPDATE ON webdock_auth.hosting_service_binding TO webdock_worker_runtime;
CREATE POLICY binding_agent ON webdock_auth.hosting_service_binding TO webdock_worker_runtime USING(webdock_rls.agent_project(project_id::text)) WITH CHECK(webdock_rls.agent_project(project_id::text));
