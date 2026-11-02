-- Native0023: only explicitly admitted native operations may reach an enrolled agent.
-- Historical pending/running rows retain their data and require reviewed reconciliation.
CREATE TABLE webdock_mail.native_operation (
 operation_id text PRIMARY KEY REFERENCES webdock_mail.operation(id) ON DELETE RESTRICT,
 customer_id text NOT NULL REFERENCES webdock_mail.service(customer_id) ON DELETE RESTRICT,
 cluster_id varchar NOT NULL REFERENCES webdock_auth.hosting_cluster(id) ON DELETE RESTRICT,
 subject text NOT NULL REFERENCES webdock_auth."user"(id) ON DELETE RESTRICT,
 session_id text NOT NULL CHECK(length(session_id) BETWEEN 1 AND 256),
 native_authority jsonb NOT NULL CHECK(jsonb_typeof(native_authority)='object' AND
  coalesce(native_authority->>'kind' IN ('local-session','first-party-browser'),false)),
 request_sha256 text CHECK(request_sha256 ~ '^[0-9a-f]{64}$'),
 spec jsonb CHECK(jsonb_typeof(spec)='object'),
 encrypted_material text,
 material_sha256 text CHECK(material_sha256 ~ '^[0-9a-f]{64}$'),
 lease_token_sha256 text CHECK(lease_token_sha256 ~ '^[0-9a-f]{64}$'),
 agent_generation integer CHECK(agent_generation>0),
 checkpoint_sha256 text CHECK(checkpoint_sha256 ~ '^[0-9a-f]{64}$'),
 checkpoint_runtime_id text,
 checkpoint_live boolean,
 receipt_sha256 text CHECK(receipt_sha256 ~ '^[0-9a-f]{64}$'),
 receipt_live boolean,
 created_at timestamptz NOT NULL DEFAULT now(),
 CHECK((request_sha256 IS NULL AND spec IS NULL AND encrypted_material IS NULL AND
  material_sha256 IS NULL AND lease_token_sha256 IS NULL AND agent_generation IS NULL) OR
  (request_sha256 IS NOT NULL AND spec IS NOT NULL AND encrypted_material IS NOT NULL AND
   material_sha256 IS NOT NULL AND lease_token_sha256 IS NOT NULL AND agent_generation IS NOT NULL)),
 CHECK((checkpoint_sha256 IS NULL)=(checkpoint_live IS NULL)),
 CHECK((receipt_sha256 IS NULL)=(receipt_live IS NULL))
);
ALTER TABLE webdock_mail.native_operation OWNER TO webdock_platform_owner;
ALTER TABLE webdock_mail.native_operation ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_mail.native_operation FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_mail.native_operation FROM PUBLIC;
GRANT INSERT(operation_id,customer_id,cluster_id,subject,session_id,native_authority)
 ON webdock_mail.native_operation TO webdock_auth_runtime;
GRANT SELECT ON webdock_mail.native_operation TO webdock_auth_runtime,webdock_worker_runtime;
GRANT UPDATE(request_sha256,spec,encrypted_material,material_sha256,lease_token_sha256,
 agent_generation,checkpoint_sha256,checkpoint_runtime_id,checkpoint_live,receipt_sha256,receipt_live)
 ON webdock_mail.native_operation TO webdock_worker_runtime;
CREATE POLICY native_mail_owner ON webdock_mail.native_operation TO webdock_platform_owner USING(true) WITH CHECK(true);
CREATE POLICY native_mail_admit ON webdock_mail.native_operation FOR INSERT TO webdock_auth_runtime
 WITH CHECK(webdock_rls.operator() AND subject=webdock_rls.subject()
  AND session_id=webdock_rls.actor()->>'sessionID'
  AND EXISTS(SELECT 1 FROM webdock_mail.operation o JOIN webdock_mail.service s ON s.customer_id=o.customer_id
   WHERE o.id=operation_id AND o.customer_id=native_operation.customer_id AND o.requested_by=subject
    AND s.host_id=cluster_id AND o.state='pending'));
CREATE POLICY native_mail_read ON webdock_mail.native_operation FOR SELECT TO webdock_auth_runtime
 USING(webdock_rls.customer_allowed(customer_id));
CREATE POLICY native_mail_agent ON webdock_mail.native_operation TO webdock_worker_runtime
 USING(webdock_rls.worker(ARRAY['agent']::text[],cluster_id::text))
 WITH CHECK(webdock_rls.worker(ARRAY['agent']::text[],cluster_id::text));

CREATE FUNCTION webdock_mail.freeze_native_operation() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 IF ROW(NEW.operation_id,NEW.customer_id,NEW.cluster_id,NEW.subject,NEW.session_id,NEW.native_authority,NEW.created_at)
  IS DISTINCT FROM ROW(OLD.operation_id,OLD.customer_id,OLD.cluster_id,OLD.subject,OLD.session_id,OLD.native_authority,OLD.created_at)
 THEN RAISE EXCEPTION 'Mail admission is immutable' USING ERRCODE='42501'; END IF;
 IF OLD.request_sha256 IS NOT NULL AND
  ROW(NEW.request_sha256,NEW.spec,NEW.encrypted_material,NEW.material_sha256,NEW.lease_token_sha256,NEW.agent_generation)
  IS DISTINCT FROM ROW(OLD.request_sha256,OLD.spec,OLD.encrypted_material,OLD.material_sha256,OLD.lease_token_sha256,OLD.agent_generation)
 THEN RAISE EXCEPTION 'Mail dispatch is immutable' USING ERRCODE='42501'; END IF;
 IF OLD.checkpoint_sha256 IS NOT NULL AND ROW(NEW.checkpoint_sha256,NEW.checkpoint_runtime_id,NEW.checkpoint_live)
  IS DISTINCT FROM ROW(OLD.checkpoint_sha256,OLD.checkpoint_runtime_id,OLD.checkpoint_live)
 THEN RAISE EXCEPTION 'Mail checkpoint is immutable' USING ERRCODE='42501'; END IF;
 IF OLD.receipt_sha256 IS NOT NULL AND ROW(NEW.receipt_sha256,NEW.receipt_live)
  IS DISTINCT FROM ROW(OLD.receipt_sha256,OLD.receipt_live)
 THEN RAISE EXCEPTION 'Mail receipt is immutable' USING ERRCODE='42501'; END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_mail.freeze_native_operation() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_mail.freeze_native_operation() FROM PUBLIC;
CREATE TRIGGER freeze_native_mail_operation BEFORE UPDATE ON webdock_mail.native_operation
 FOR EACH ROW EXECUTE FUNCTION webdock_mail.freeze_native_operation();
