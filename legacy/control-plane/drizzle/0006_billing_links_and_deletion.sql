-- Bind every paid website to its registry project and preserve contract identity throughout payment records.
ALTER TABLE webdock_auth.billing_contract ALTER COLUMN project_id SET NOT NULL;
ALTER TABLE webdock_auth.billing_checkout ADD UNIQUE(environment,customer_id,contract_id,id);
ALTER TABLE webdock_auth.billing_subscription ADD FOREIGN KEY(environment,customer_id,contract_id,checkout_id) REFERENCES webdock_auth.billing_checkout(environment,customer_id,contract_id,id) ON DELETE RESTRICT;
ALTER TABLE webdock_auth.billing_subscription ADD UNIQUE(environment,customer_id,contract_id,id);
ALTER TABLE webdock_auth.billing_order ADD FOREIGN KEY(environment,customer_id,contract_id,subscription_id) REFERENCES webdock_auth.billing_subscription(environment,customer_id,contract_id,id) ON DELETE RESTRICT;
GRANT USAGE,CREATE ON SCHEMA webdock_rls TO webdock_platform_owner;
CREATE FUNCTION webdock_rls.project_without_billing(project text) RETURNS void LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
BEGIN
 IF NOT webdock_rls.operator() THEN RAISE EXCEPTION 'Operator required' USING ERRCODE='42501';END IF;
 PERFORM pg_advisory_xact_lock(hashtextextended('webdock-billing-project:'||project,0));
 IF EXISTS(SELECT 1 FROM webdock_auth.billing_contract WHERE project_id=project) THEN RAISE EXCEPTION 'Billing history prevents project deletion' USING ERRCODE='P0001';END IF;
END $$;
ALTER FUNCTION webdock_rls.project_without_billing(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.project_without_billing(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.project_without_billing(text) TO webdock_registry_runtime;
CREATE FUNCTION webdock_auth.billing_project_fence() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
BEGIN
 PERFORM pg_advisory_xact_lock(hashtextextended('webdock-billing-project:'||NEW.project_id,0));
 IF EXISTS(SELECT 1 FROM webdock_admin.project_deletion WHERE project_id=NEW.project_id) THEN RAISE EXCEPTION 'Project deletion prevents a billing contract' USING ERRCODE='55000';END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_auth.billing_project_fence() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.billing_project_fence() FROM PUBLIC;
CREATE TRIGGER billing_contract_project_fence BEFORE INSERT ON webdock_auth.billing_contract FOR EACH ROW EXECUTE FUNCTION webdock_auth.billing_project_fence();
REVOKE CREATE ON SCHEMA webdock_rls FROM webdock_platform_owner;
