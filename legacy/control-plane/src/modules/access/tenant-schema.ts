import { tenantOwnerSchemaSQL } from "./tenant-owner-schema";
/** Run with the offline database owner; never grant Studio direct auth-table access. */
const tenantBaseSchemaSQL = `
CREATE TABLE IF NOT EXISTS webdock_auth.tenant_customer (
 customer_id varchar PRIMARY KEY REFERENCES webdock_admin.customers(id) ON DELETE RESTRICT,
 organization_id text NOT NULL UNIQUE REFERENCES webdock_auth.organization(id) ON DELETE RESTRICT
);
CREATE OR REPLACE FUNCTION webdock_auth.sync_customer_tenant() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE tenant_id text;
BEGIN
 SELECT organization_id INTO tenant_id FROM webdock_auth.tenant_customer WHERE customer_id=NEW.id;
 IF tenant_id IS NULL THEN
   tenant_id := webdock_auth.next_snowflake();
   INSERT INTO webdock_auth.organization(id,name,slug,"createdAt")
   VALUES(tenant_id,NEW.name,'customer-' || NEW.id,now());
   INSERT INTO webdock_auth.tenant_customer(customer_id,organization_id) VALUES(NEW.id,tenant_id);
 ELSE
   UPDATE webdock_auth.organization SET name=NEW.name WHERE id=tenant_id;
 END IF;
 RETURN NEW;
END;
$$;
REVOKE ALL ON FUNCTION webdock_auth.sync_customer_tenant() FROM PUBLIC;
DROP TRIGGER IF EXISTS webdock_customer_tenant ON webdock_admin.customers;
CREATE TRIGGER webdock_customer_tenant AFTER INSERT OR UPDATE OF name ON webdock_admin.customers
FOR EACH ROW EXECUTE FUNCTION webdock_auth.sync_customer_tenant();
`;

export const tenantProfileColumns = [
  "name",
  "contact_name",
  "contact_email",
  "customer_type",
  "first_name",
  "last_name",
  "company_name",
  "phone",
  "address_line1",
  "address_line2",
  "postal_code",
  "city",
  "region",
  "country",
] as const;

/** Identifiers are allowlisted, including the runtime role supplied by the offline runner. */
export function tenantProfileGrants(role: string) {
  if (!/^[a-z][a-z0-9_]{0,62}$/.test(role)) throw Error("Invalid runtime role");
  const write = [...tenantProfileColumns, "updated_at"].join(",");
  return `GRANT USAGE ON SCHEMA webdock_admin TO "${role}";
GRANT SELECT(id,status,created_at,${write}), UPDATE(${write}) ON webdock_admin.customers TO "${role}";
GRANT SELECT, REFERENCES(customer_id) ON webdock_auth.tenant_customer TO "${role}";`;
}

export const tenantSchemaSQL = tenantBaseSchemaSQL + tenantOwnerSchemaSQL;
