-- Native0025 draft: operators can delete an archived customer that owns nothing else.
ALTER TYPE webdock_admin.enum_audit_events_action ADD VALUE IF NOT EXISTS 'delete';
-- The tenant link restricts customer deletion. Release it with the customer, and remove the tenant
-- organization (its members and invitations cascade) unless other records still refer to it.
CREATE OR REPLACE FUNCTION webdock_auth.release_customer_tenant() RETURNS trigger
 LANGUAGE plpgsql SECURITY DEFINER SET search_path TO 'pg_catalog' AS $$
DECLARE tenant_id text;
BEGIN
 DELETE FROM webdock_auth.tenant_customer WHERE customer_id=OLD.id RETURNING organization_id INTO tenant_id;
 IF tenant_id IS NOT NULL THEN
  BEGIN
   DELETE FROM webdock_auth.organization WHERE id=tenant_id;
  EXCEPTION WHEN foreign_key_violation THEN
   NULL; -- Grants, bindings or activation evidence still refer to it; keep the organization.
  END;
 END IF;
 RETURN OLD;
END $$;
ALTER FUNCTION webdock_auth.release_customer_tenant() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.release_customer_tenant() FROM PUBLIC;
DROP TRIGGER IF EXISTS webdock_customer_release ON webdock_admin.customers;
CREATE TRIGGER webdock_customer_release BEFORE DELETE ON webdock_admin.customers FOR EACH ROW EXECUTE FUNCTION webdock_auth.release_customer_tenant();
