-- Installation operator supplements use their own immutable receipt, never a product job.
-- Subscription snapshots/history and the existing paid-base guard remain unchanged.
CREATE TABLE webdock_auth.allowance_operation (
 id text PRIMARY KEY,
 actor_id text NOT NULL,
 customer_id varchar NOT NULL REFERENCES webdock_auth.tenant_customer(customer_id) ON DELETE RESTRICT,
 idempotency_key text NOT NULL CHECK (length(idempotency_key) BETWEEN 1 AND 128),
 payload_sha256 text NOT NULL CHECK (payload_sha256 ~ '^[a-f0-9]{64}$'),
 result jsonb NOT NULL CHECK (jsonb_typeof(result)='object' AND octet_length(result::text)<=16384),
 created_at timestamptz NOT NULL DEFAULT statement_timestamp(),
 UNIQUE(actor_id,idempotency_key)
);
ALTER TABLE webdock_auth.allowance_operation OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.allowance_operation ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.allowance_operation FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_auth.allowance_operation FROM PUBLIC,webdock_auth_runtime,webdock_worker_runtime;
GRANT SELECT,INSERT ON webdock_auth.allowance_operation TO webdock_auth_runtime;
CREATE POLICY platform_owner ON webdock_auth.allowance_operation TO webdock_platform_owner
 USING (true) WITH CHECK (true);
CREATE POLICY auth_read ON webdock_auth.allowance_operation FOR SELECT TO webdock_auth_runtime
 USING (actor_id=webdock_rls.subject() AND webdock_rls.operator()
  AND webdock_rls.customer_permission(customer_id,'org:billing:manage'));
CREATE POLICY auth_insert ON webdock_auth.allowance_operation FOR INSERT TO webdock_auth_runtime
 WITH CHECK (actor_id=webdock_rls.subject() AND webdock_rls.operator()
  AND webdock_rls.customer_permission(customer_id,'org:billing:manage'));

-- No role, including the migration owner, can rewrite a retained successful result.
CREATE FUNCTION webdock_auth.guard_allowance_receipt() RETURNS trigger
 LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 RAISE EXCEPTION 'Allowance receipts are immutable' USING ERRCODE='55000';
END $$;
ALTER FUNCTION webdock_auth.guard_allowance_receipt() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.guard_allowance_receipt() FROM PUBLIC;
CREATE TRIGGER allowance_receipt_immutable BEFORE UPDATE OR DELETE
 ON webdock_auth.allowance_operation FOR EACH ROW
 EXECUTE FUNCTION webdock_auth.guard_allowance_receipt();
