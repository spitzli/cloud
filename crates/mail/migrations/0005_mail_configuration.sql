-- Native0024 draft: original configuration authority and exact external-mutation custody.
-- Historical jobs have no admission row and cannot be claimed by the native executor.
CREATE TABLE webdock_mail.native_configuration_operation (
 operation_id text PRIMARY KEY REFERENCES webdock_mail.configuration_operation(id) ON DELETE RESTRICT,
 customer_id text NOT NULL REFERENCES webdock_mail.service(customer_id) ON DELETE RESTRICT,
 cluster_id varchar NOT NULL REFERENCES webdock_auth.hosting_cluster(id) ON DELETE RESTRICT,
 provenance_kind text NOT NULL DEFAULT 'human' CHECK(provenance_kind IN ('human','restriction')),
 mode text NOT NULL DEFAULT 'apply' CHECK(mode IN ('apply','restrict')),
 subject text REFERENCES webdock_auth."user"(id) ON DELETE RESTRICT,
 session_id text CHECK(length(session_id) BETWEEN 1 AND 256),
 native_authority jsonb,
 admission_member_id text CHECK(length(admission_member_id) BETWEEN 1 AND 256),
 operator_authority boolean NOT NULL DEFAULT false,
 restricted_mailbox_id text,
 restricted_subject text,
 restricted_revision bigint CHECK(restricted_revision>0),
 request_sha256 text CHECK(request_sha256 ~ '^[0-9a-f]{64}$'),
 spec jsonb CHECK(jsonb_typeof(spec)='object'),
 encrypted_material text,
 material_sha256 text CHECK(material_sha256 ~ '^[0-9a-f]{64}$'),
 lease_token_sha256 text CHECK(lease_token_sha256 ~ '^[0-9a-f]{64}$'),
 agent_generation integer CHECK(agent_generation>0),
 runtime_id text CHECK(length(runtime_id) BETWEEN 1 AND 128),
 receipt_sha256 text CHECK(receipt_sha256 ~ '^[0-9a-f]{64}$'),
 receipt_live boolean,
 created_at timestamptz NOT NULL DEFAULT now(),
 CHECK((provenance_kind='human' AND subject IS NOT NULL AND session_id IS NOT NULL
   AND ((operator_authority AND admission_member_id IS NULL) OR (NOT operator_authority AND admission_member_id IS NOT NULL))
   AND jsonb_typeof(native_authority)='object' AND coalesce(native_authority->>'kind' IN ('local-session','first-party-browser'),false)
   AND ((mode='apply' AND restricted_mailbox_id IS NULL AND restricted_subject IS NULL AND restricted_revision IS NULL)
    OR (mode='restrict' AND restricted_mailbox_id IS NOT NULL AND restricted_subject IS NOT NULL AND restricted_revision IS NOT NULL)))
  OR (provenance_kind='restriction' AND mode='restrict' AND NOT operator_authority AND admission_member_id IS NULL AND subject IS NULL AND session_id IS NULL AND native_authority IS NULL
   AND restricted_mailbox_id IS NOT NULL AND restricted_subject IS NOT NULL AND restricted_revision IS NOT NULL)),
 CHECK((request_sha256 IS NULL AND spec IS NULL AND encrypted_material IS NULL AND material_sha256 IS NULL AND lease_token_sha256 IS NULL AND agent_generation IS NULL)
  OR (request_sha256 IS NOT NULL AND spec IS NOT NULL AND encrypted_material IS NOT NULL AND material_sha256 IS NOT NULL AND lease_token_sha256 IS NOT NULL AND agent_generation IS NOT NULL)),
 CHECK((receipt_sha256 IS NULL)=(receipt_live IS NULL))
);
ALTER TABLE webdock_mail.native_configuration_operation OWNER TO webdock_platform_owner;
ALTER TABLE webdock_mail.native_configuration_operation ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_mail.native_configuration_operation FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_mail.native_configuration_operation FROM PUBLIC;
GRANT INSERT(operation_id,customer_id,cluster_id,subject,session_id,native_authority,mode,restricted_mailbox_id,restricted_subject,restricted_revision,admission_member_id,operator_authority) ON webdock_mail.native_configuration_operation TO webdock_auth_runtime;
GRANT SELECT ON webdock_mail.native_configuration_operation TO webdock_auth_runtime,webdock_worker_runtime;
GRANT UPDATE(request_sha256,spec,encrypted_material,material_sha256,lease_token_sha256,agent_generation,runtime_id,receipt_sha256,receipt_live) ON webdock_mail.native_configuration_operation TO webdock_worker_runtime;
CREATE POLICY native_configuration_owner ON webdock_mail.native_configuration_operation TO webdock_platform_owner USING(true) WITH CHECK(true);
CREATE POLICY native_configuration_admit ON webdock_mail.native_configuration_operation FOR INSERT TO webdock_auth_runtime
 WITH CHECK(provenance_kind='human' AND webdock_rls.customer_allowed(customer_id,true) AND subject=webdock_rls.subject()
  AND session_id=webdock_rls.actor()->>'sessionID'
  AND ((operator_authority AND webdock_rls.operator()) OR (NOT operator_authority AND EXISTS(SELECT 1 FROM webdock_auth.member m JOIN webdock_auth.tenant_customer t ON t.organization_id=m."organizationId" WHERE m.id=admission_member_id AND m."userId"=subject AND t.customer_id=native_configuration_operation.customer_id AND m.role IN ('owner','admin'))))
  AND EXISTS(SELECT 1 FROM webdock_mail.configuration_operation o JOIN webdock_mail.service s ON s.customer_id=o.customer_id
   WHERE o.id=operation_id AND o.customer_id=native_configuration_operation.customer_id AND o.requested_by=subject AND s.host_id=cluster_id AND o.state='pending'));
CREATE POLICY native_configuration_read ON webdock_mail.native_configuration_operation FOR SELECT TO webdock_auth_runtime USING(webdock_rls.customer_allowed(customer_id));
CREATE POLICY native_configuration_agent ON webdock_mail.native_configuration_operation TO webdock_worker_runtime
 USING(webdock_rls.worker(ARRAY['agent']::text[],cluster_id::text)) WITH CHECK(webdock_rls.worker(ARRAY['agent']::text[],cluster_id::text));

CREATE TABLE webdock_mail.native_configuration_checkpoint (
 operation_id text NOT NULL REFERENCES webdock_mail.native_configuration_operation(operation_id) ON DELETE RESTRICT,
 ordinal integer NOT NULL CHECK(ordinal BETWEEN 1 AND 449),
 event_key text NOT NULL CHECK(length(event_key) BETWEEN 1 AND 80),
 callback_sha256 text NOT NULL CHECK(callback_sha256 ~ '^[0-9a-f]{64}$'),
 checkpoint jsonb NOT NULL CHECK(jsonb_typeof(checkpoint)='object'),
 live boolean NOT NULL,
 PRIMARY KEY(operation_id,event_key),UNIQUE(operation_id,ordinal),UNIQUE(operation_id,callback_sha256)
);
ALTER TABLE webdock_mail.native_configuration_checkpoint OWNER TO webdock_platform_owner;
ALTER TABLE webdock_mail.native_configuration_checkpoint ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_mail.native_configuration_checkpoint FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_mail.native_configuration_checkpoint FROM PUBLIC;
GRANT SELECT,INSERT ON webdock_mail.native_configuration_checkpoint TO webdock_worker_runtime;
CREATE POLICY native_configuration_checkpoint_owner ON webdock_mail.native_configuration_checkpoint TO webdock_platform_owner USING(true) WITH CHECK(true);
CREATE POLICY native_configuration_checkpoint_agent ON webdock_mail.native_configuration_checkpoint TO webdock_worker_runtime
 USING(EXISTS(SELECT 1 FROM webdock_mail.native_configuration_operation n WHERE n.operation_id=native_configuration_checkpoint.operation_id AND webdock_rls.worker(ARRAY['agent']::text[],n.cluster_id::text)))
 WITH CHECK(EXISTS(SELECT 1 FROM webdock_mail.native_configuration_operation n WHERE n.operation_id=native_configuration_checkpoint.operation_id AND webdock_rls.worker(ARRAY['agent']::text[],n.cluster_id::text)));

CREATE FUNCTION webdock_mail.freeze_native_configuration() RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 IF ROW(NEW.operation_id,NEW.customer_id,NEW.cluster_id,NEW.provenance_kind,NEW.mode,NEW.subject,NEW.session_id,NEW.native_authority,NEW.admission_member_id,NEW.operator_authority,NEW.restricted_mailbox_id,NEW.restricted_subject,NEW.restricted_revision,NEW.created_at)
 IS DISTINCT FROM ROW(OLD.operation_id,OLD.customer_id,OLD.cluster_id,OLD.provenance_kind,OLD.mode,OLD.subject,OLD.session_id,OLD.native_authority,OLD.admission_member_id,OLD.operator_authority,OLD.restricted_mailbox_id,OLD.restricted_subject,OLD.restricted_revision,OLD.created_at)
 THEN RAISE EXCEPTION 'Configuration admission is immutable' USING ERRCODE='42501'; END IF;
 IF OLD.request_sha256 IS NOT NULL AND ROW(NEW.request_sha256,NEW.spec,NEW.encrypted_material,NEW.material_sha256,NEW.lease_token_sha256,NEW.agent_generation)
 IS DISTINCT FROM ROW(OLD.request_sha256,OLD.spec,OLD.encrypted_material,OLD.material_sha256,OLD.lease_token_sha256,OLD.agent_generation)
 THEN RAISE EXCEPTION 'Configuration dispatch is immutable' USING ERRCODE='42501'; END IF;
 IF OLD.runtime_id IS NOT NULL AND NEW.runtime_id IS DISTINCT FROM OLD.runtime_id
 THEN RAISE EXCEPTION 'Configuration runtime is immutable' USING ERRCODE='42501'; END IF;
 IF OLD.receipt_sha256 IS NOT NULL AND ROW(NEW.receipt_sha256,NEW.receipt_live) IS DISTINCT FROM ROW(OLD.receipt_sha256,OLD.receipt_live)
 THEN RAISE EXCEPTION 'Configuration receipt is immutable' USING ERRCODE='42501'; END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_mail.freeze_native_configuration() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_mail.freeze_native_configuration() FROM PUBLIC;
CREATE TRIGGER freeze_native_configuration BEFORE UPDATE ON webdock_mail.native_configuration_operation FOR EACH ROW EXECUTE FUNCTION webdock_mail.freeze_native_configuration();

-- SQL triggers preserve a restriction event without reviving the disabled legacy ID allocator.
-- The native worker allocates its eventual operation ID through the existing Rust allocator.
CREATE TABLE webdock_mail.native_configuration_restriction (
 customer_id text NOT NULL REFERENCES webdock_mail.service(customer_id) ON DELETE RESTRICT,
 configuration_revision bigint NOT NULL CHECK(configuration_revision>0),
 mailbox_id text NOT NULL,subject text NOT NULL,grant_revision bigint NOT NULL CHECK(grant_revision>0),
 operation_id text UNIQUE REFERENCES webdock_mail.native_configuration_operation(operation_id) ON DELETE RESTRICT,
 obsolete boolean NOT NULL DEFAULT false CHECK(NOT obsolete OR operation_id IS NULL),
 created_at timestamptz NOT NULL DEFAULT now(),PRIMARY KEY(customer_id,configuration_revision)
);
ALTER TABLE webdock_mail.native_configuration_restriction OWNER TO webdock_platform_owner;
ALTER TABLE webdock_mail.native_configuration_restriction ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_mail.native_configuration_restriction FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_mail.native_configuration_restriction FROM PUBLIC;
GRANT SELECT ON webdock_mail.native_configuration_restriction TO webdock_worker_runtime;
CREATE POLICY native_configuration_restriction_owner ON webdock_mail.native_configuration_restriction TO webdock_platform_owner USING(true) WITH CHECK(true);
CREATE POLICY native_configuration_restriction_agent ON webdock_mail.native_configuration_restriction FOR SELECT TO webdock_worker_runtime
 USING(EXISTS(SELECT 1 FROM webdock_mail.service s WHERE s.customer_id=native_configuration_restriction.customer_id AND webdock_rls.worker(ARRAY['agent']::text[],s.host_id::text)));

CREATE FUNCTION webdock_mail.freeze_configuration_restriction() RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 IF ROW(NEW.customer_id,NEW.configuration_revision,NEW.mailbox_id,NEW.subject,NEW.grant_revision,NEW.created_at)
 IS DISTINCT FROM ROW(OLD.customer_id,OLD.configuration_revision,OLD.mailbox_id,OLD.subject,OLD.grant_revision,OLD.created_at)
 OR (OLD.operation_id IS NOT NULL AND NEW.operation_id IS DISTINCT FROM OLD.operation_id)
 OR (OLD.obsolete AND NOT NEW.obsolete)
 THEN RAISE EXCEPTION 'Configuration restriction is immutable' USING ERRCODE='42501'; END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_mail.freeze_configuration_restriction() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_mail.freeze_configuration_restriction() FROM PUBLIC;
CREATE TRIGGER freeze_configuration_restriction BEFORE UPDATE ON webdock_mail.native_configuration_restriction FOR EACH ROW EXECUTE FUNCTION webdock_mail.freeze_configuration_restriction();

-- No caller-selected user authority is accepted; only the exact atomically recorded contraction.
CREATE FUNCTION webdock_mail.admit_configuration_restriction(requested_customer text,requested_revision bigint,requested_operation text) RETURNS boolean
 LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE event webdock_mail.native_configuration_restriction%ROWTYPE; cluster varchar;
BEGIN
 SELECT s.host_id INTO cluster FROM webdock_mail.service s WHERE s.customer_id=requested_customer;
 IF NOT coalesce(webdock_rls.worker(ARRAY['agent']::text[],cluster::text),false)
 THEN RAISE EXCEPTION 'Enrolled configuration agent required' USING ERRCODE='42501'; END IF;
 IF requested_operation !~ '^[1-9][0-9]{0,18}$' OR requested_operation::numeric>9223372036854775807
 THEN RAISE EXCEPTION 'Invalid native operation identity' USING ERRCODE='22023'; END IF;
 SELECT * INTO event FROM webdock_mail.native_configuration_restriction e WHERE e.customer_id=requested_customer AND e.configuration_revision=requested_revision FOR UPDATE;
 IF NOT FOUND OR event.operation_id IS NOT NULL OR event.obsolete THEN RETURN false; END IF;
 IF NOT EXISTS(SELECT 1 FROM webdock_mail.configuration c WHERE c.customer_id=requested_customer AND c.revision>=requested_revision) THEN RETURN false; END IF;
 IF NOT EXISTS(SELECT 1 FROM webdock_mail.mailbox_grant g WHERE g.customer_id=requested_customer AND g.mailbox_id=event.mailbox_id AND g.subject=event.subject AND NOT g.enabled AND g.revision=event.grant_revision AND g.member_id IS NULL AND g.clerk_grant IS NULL)
 THEN UPDATE webdock_mail.native_configuration_restriction e SET obsolete=true WHERE e.customer_id=requested_customer AND e.configuration_revision=requested_revision; RETURN false; END IF;
 INSERT INTO webdock_mail.configuration_operation(id,customer_id,revision,requested_by,reason)
 VALUES(requested_operation,requested_customer,requested_revision,NULL,'membership_revoked');
 INSERT INTO webdock_mail.native_configuration_operation(operation_id,customer_id,cluster_id,provenance_kind,mode,restricted_mailbox_id,restricted_subject,restricted_revision)
 VALUES(requested_operation,requested_customer,cluster,'restriction','restrict',event.mailbox_id,event.subject,event.grant_revision);
 UPDATE webdock_mail.native_configuration_restriction e SET operation_id=requested_operation WHERE e.customer_id=requested_customer AND e.configuration_revision=requested_revision;
 RETURN true;
END $$;
ALTER FUNCTION webdock_mail.admit_configuration_restriction(text,bigint,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_mail.admit_configuration_restriction(text,bigint,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_mail.admit_configuration_restriction(text,bigint,text) TO webdock_worker_runtime;

CREATE OR REPLACE FUNCTION webdock_mail.revoke_detached_member() RETURNS trigger
 LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE next_revision bigint;
BEGIN
 IF TG_TABLE_SCHEMA<>'webdock_mail' OR TG_TABLE_NAME<>'mailbox_grant' OR TG_OP<>'UPDATE'
  OR OLD.member_id IS NULL OR NEW.member_id IS NOT NULL OR NEW.clerk_grant IS NOT NULL
 THEN RAISE EXCEPTION 'Invalid membership restriction trigger' USING ERRCODE='42501'; END IF;
 IF TG_WHEN='BEFORE' THEN NEW.enabled:=false; NEW.revision:=OLD.revision+1; RETURN NEW; END IF;
 INSERT INTO webdock_mail.configuration(customer_id) VALUES(NEW.customer_id)
 ON CONFLICT(customer_id) DO UPDATE SET revision=configuration.revision+1 RETURNING revision INTO next_revision;
 UPDATE webdock_mail.configuration_operation o SET state='superseded',finished_at=now()
 WHERE o.customer_id=NEW.customer_id AND o.state='pending' AND NOT EXISTS(SELECT 1 FROM webdock_mail.native_configuration_operation n WHERE n.operation_id=o.id AND n.mode='restrict');
 INSERT INTO webdock_mail.native_configuration_restriction(customer_id,configuration_revision,mailbox_id,subject,grant_revision)
 VALUES(NEW.customer_id,next_revision,NEW.mailbox_id,NEW.subject,NEW.revision);
 RETURN NULL;
END $$;
ALTER FUNCTION webdock_mail.revoke_detached_member() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_mail.revoke_detached_member() FROM PUBLIC;
