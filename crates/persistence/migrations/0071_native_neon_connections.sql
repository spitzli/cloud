-- Owner-selected native BYOK metadata lives in Core; no provider-owned metadata service.
GRANT CREATE ON SCHEMA webdock_rls TO webdock_platform_owner;
CREATE FUNCTION webdock_rls.neon_connection_allowed(customer text, writing boolean DEFAULT true) RETURNS boolean
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
 SELECT webdock_rls.customer_allowed(customer,writing) AND
 (webdock_rls.customer_permission(customer,'org:databases:manage') OR
  (NOT writing AND webdock_rls.actor()->'preview'<>'null'::jsonb AND
   webdock_rls.actor()->'preview'->>'customer_id'=customer))
$$;
ALTER FUNCTION webdock_rls.neon_connection_allowed(text,boolean) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.neon_connection_allowed(text,boolean) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.neon_connection_allowed(text,boolean) TO webdock_auth_runtime;
REVOKE CREATE ON SCHEMA webdock_rls FROM webdock_platform_owner;
CREATE TABLE webdock_auth.neon_connection (
 customer_id text PRIMARY KEY REFERENCES webdock_admin.customers(id) ON DELETE CASCADE,
 id uuid NOT NULL UNIQUE,
 revision bigint NOT NULL CHECK(revision BETWEEN 1 AND 9007199254740991),
 status text NOT NULL CHECK(status IN ('connected','disconnected')),
 organization_id text CHECK(organization_id ~ '^[a-z0-9]([a-z0-9-]{0,58}[a-z0-9])?$'),
 encrypted_key text CHECK(octet_length(encrypted_key) BETWEEN 1 AND 32768),
 connected_at timestamptz NOT NULL DEFAULT statement_timestamp(),
 updated_at timestamptz NOT NULL DEFAULT statement_timestamp(),
 CHECK((status='connected')=(encrypted_key IS NOT NULL))
);
CREATE TABLE webdock_auth.neon_connection_receipt (
 customer_id text NOT NULL REFERENCES webdock_auth.neon_connection(customer_id) ON DELETE CASCADE,
 idempotency_key text NOT NULL CHECK(idempotency_key ~ '^[A-Za-z0-9_-]{16,128}$'),
 payload_digest text NOT NULL CHECK(payload_digest ~ '^[0-9a-f]{64}$'),
 revision bigint NOT NULL CHECK(revision BETWEEN 1 AND 9007199254740991),
 created_at timestamptz NOT NULL DEFAULT statement_timestamp(),
 PRIMARY KEY(customer_id,idempotency_key)
);
ALTER TABLE webdock_auth.neon_connection OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.neon_connection_receipt OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.neon_connection ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.neon_connection FORCE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.neon_connection_receipt ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.neon_connection_receipt FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_auth.neon_connection,webdock_auth.neon_connection_receipt FROM PUBLIC,webdock_auth_bootstrap,webdock_worker_runtime,webdock_registry_runtime;
GRANT SELECT,INSERT,UPDATE ON webdock_auth.neon_connection TO webdock_auth_runtime;
GRANT SELECT,INSERT ON webdock_auth.neon_connection_receipt TO webdock_auth_runtime;
CREATE POLICY owner_all ON webdock_auth.neon_connection TO webdock_platform_owner USING(true) WITH CHECK(true);
CREATE POLICY owner_all ON webdock_auth.neon_connection_receipt TO webdock_platform_owner USING(true) WITH CHECK(true);
CREATE POLICY connection_read ON webdock_auth.neon_connection FOR SELECT TO webdock_auth_runtime USING(webdock_rls.neon_connection_allowed(customer_id,false));
CREATE POLICY connection_insert ON webdock_auth.neon_connection FOR INSERT TO webdock_auth_runtime WITH CHECK(webdock_rls.neon_connection_allowed(customer_id,true));
CREATE POLICY connection_update ON webdock_auth.neon_connection FOR UPDATE TO webdock_auth_runtime USING(webdock_rls.neon_connection_allowed(customer_id,true)) WITH CHECK(webdock_rls.neon_connection_allowed(customer_id,true));
CREATE POLICY receipt_read ON webdock_auth.neon_connection_receipt FOR SELECT TO webdock_auth_runtime USING(webdock_rls.neon_connection_allowed(customer_id,true));
CREATE POLICY receipt_insert ON webdock_auth.neon_connection_receipt FOR INSERT TO webdock_auth_runtime WITH CHECK(webdock_rls.neon_connection_allowed(customer_id,true));
-- Disconnect clears ciphertext but never removes the tombstone used to resolve lost replies.
CREATE FUNCTION webdock_auth.guard_neon_connection() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog AS $$ BEGIN
 IF NEW.customer_id IS DISTINCT FROM OLD.customer_id OR NEW.id IS DISTINCT FROM OLD.id
 OR NEW.revision<>OLD.revision+1 THEN RAISE EXCEPTION 'Connection revision required' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_auth.guard_neon_connection() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.guard_neon_connection() FROM PUBLIC;
CREATE TRIGGER neon_connection_revision BEFORE UPDATE ON webdock_auth.neon_connection FOR EACH ROW EXECUTE FUNCTION webdock_auth.guard_neon_connection();
-- Ordinary disconnect retains receipts. Only the existing permanent customer deletion can erase them.
CREATE FUNCTION webdock_auth.guard_neon_connection_receipt() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$ BEGIN
 IF TG_OP='DELETE' AND NOT EXISTS(SELECT 1 FROM webdock_admin.customers WHERE id=OLD.customer_id) THEN RETURN OLD; END IF;
 RAISE EXCEPTION 'Connection receipts are immutable' USING ERRCODE='55000';
END $$;
ALTER FUNCTION webdock_auth.guard_neon_connection_receipt() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.guard_neon_connection_receipt() FROM PUBLIC;
CREATE TRIGGER neon_connection_receipt_immutable BEFORE UPDATE OR DELETE ON webdock_auth.neon_connection_receipt FOR EACH ROW EXECUTE FUNCTION webdock_auth.guard_neon_connection_receipt();
