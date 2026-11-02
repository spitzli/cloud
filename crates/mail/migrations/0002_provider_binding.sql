-- Explicit tenant-domain approval; never fabricate a project grant for Mail.
-- Existing selected transports remain unbound until an authorized, audited adoption.
CREATE TABLE webdock_mail.provider_binding (
 customer_id text PRIMARY KEY REFERENCES webdock_admin.customers(id),
 plugin_id text NOT NULL REFERENCES webdock_auth.plugin_installation(id),
 revision bigint NOT NULL CHECK(revision>0),
 transport_revision bigint NOT NULL CHECK(transport_revision>0),
 revoked_at timestamptz,
 updated_at timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE webdock_mail.provider_binding OWNER TO webdock_platform_owner;
ALTER TABLE webdock_mail.provider_binding ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_mail.provider_binding FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_mail.provider_binding FROM PUBLIC;
GRANT SELECT,INSERT,UPDATE ON webdock_mail.provider_binding TO webdock_auth_runtime;
GRANT SELECT ON webdock_mail.provider_binding TO webdock_worker_runtime;
CREATE POLICY platform_owner ON webdock_mail.provider_binding TO webdock_platform_owner USING(true) WITH CHECK(true);
CREATE POLICY native_mail_binding_read ON webdock_mail.provider_binding FOR SELECT TO webdock_auth_runtime USING(webdock_rls.customer_allowed(customer_id,false));
CREATE POLICY native_mail_binding_create ON webdock_mail.provider_binding FOR INSERT TO webdock_auth_runtime WITH CHECK(webdock_rls.customer_allowed(customer_id,true));
CREATE POLICY native_mail_binding_change ON webdock_mail.provider_binding FOR UPDATE TO webdock_auth_runtime USING(webdock_rls.customer_allowed(customer_id,true)) WITH CHECK(webdock_rls.customer_allowed(customer_id,true));
CREATE POLICY native_mail_binding_worker_read ON webdock_mail.provider_binding FOR SELECT TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['mail-send-gateway']));
CREATE FUNCTION webdock_mail.advance_provider_binding() RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 IF NEW.customer_id IS DISTINCT FROM OLD.customer_id OR NEW.revision IS DISTINCT FROM OLD.revision+1 THEN
  RAISE EXCEPTION 'Mail provider binding revision required' USING ERRCODE='23514';
 END IF;
 NEW.updated_at:=clock_timestamp();
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_mail.advance_provider_binding() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_mail.advance_provider_binding() FROM PUBLIC;
CREATE TRIGGER provider_binding_revision BEFORE UPDATE ON webdock_mail.provider_binding FOR EACH ROW EXECUTE FUNCTION webdock_mail.advance_provider_binding();
