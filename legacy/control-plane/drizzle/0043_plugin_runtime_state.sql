ALTER TABLE webdock_auth.plugin_installation
 ADD COLUMN current_generation bigint CHECK(current_generation>0),
 ADD COLUMN candidate_generation bigint CHECK(candidate_generation>0),
 ADD COLUMN previous_generation bigint CHECK(previous_generation>0);
CREATE TABLE webdock_auth.plugin_configuration (
 plugin_id text PRIMARY KEY REFERENCES webdock_auth.plugin_installation(id),
 revision bigint NOT NULL DEFAULT 0 CHECK(revision>=0),
 approved_capabilities jsonb NOT NULL DEFAULT '[]' CHECK(jsonb_typeof(approved_capabilities)='array'),
 configuration jsonb NOT NULL DEFAULT '{}' CHECK(jsonb_typeof(configuration)='object' AND octet_length(configuration::text)<=65536),
 secret_refs jsonb NOT NULL DEFAULT '{}' CHECK(jsonb_typeof(secret_refs)='object'), updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE webdock_auth.plugin_runtime (
 plugin_id text NOT NULL REFERENCES webdock_auth.plugin_installation(id), generation bigint NOT NULL CHECK(generation>0),
 version text NOT NULL, image text NOT NULL CHECK(image ~ '@sha256:[a-f0-9]{64}$'),
 status text NOT NULL CHECK(status IN ('prepared','starting','ready','draining','stopped','failed')),
 configuration_revision bigint NOT NULL CHECK(configuration_revision>=0), credential_hash text CHECK(credential_hash ~ '^[a-f0-9]{64}$'),
 cluster_id text, namespace text, service_name text, operation_id text REFERENCES webdock_auth.plugin_operation(id),
 created_at timestamptz NOT NULL DEFAULT now(), observed_at timestamptz,
 PRIMARY KEY(plugin_id,generation), FOREIGN KEY(plugin_id,version) REFERENCES webdock_auth.plugin_release(plugin_id,version)
);
CREATE TABLE webdock_auth.plugin_participant (
 id uuid PRIMARY KEY, kind text NOT NULL CHECK(kind IN ('api','worker','controller','agent')),
 protocol integer NOT NULL CHECK(protocol=1), domains jsonb NOT NULL CHECK(jsonb_typeof(domains)='array'),
 generation bigint NOT NULL CHECK(generation>0), heartbeat_at timestamptz NOT NULL DEFAULT now(), expires_at timestamptz NOT NULL,
 CHECK(expires_at>heartbeat_at AND expires_at<=heartbeat_at+interval '90 seconds')
);
CREATE TABLE webdock_auth.plugin_job_execution (
 id text PRIMARY KEY DEFAULT webdock_auth.next_snowflake(), plugin_id text NOT NULL REFERENCES webdock_auth.plugin_installation(id),
 customer_id text NOT NULL, project_id text NOT NULL, name text NOT NULL, slot timestamptz,
 idempotency_key text, payload jsonb NOT NULL DEFAULT '{}' CHECK(octet_length(payload::text)<=32768),
 grant_revision bigint NOT NULL CHECK(grant_revision>=0), runtime_generation bigint NOT NULL CHECK(runtime_generation>0),
 status text NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','running','succeeded','failed','cancelled')),
 claim_token uuid, lease_until timestamptz, attempts integer NOT NULL DEFAULT 0 CHECK(attempts BETWEEN 0 AND 8),
 cancel_requested boolean NOT NULL DEFAULT false, available_at timestamptz NOT NULL DEFAULT now(), created_at timestamptz NOT NULL DEFAULT now(),
 UNIQUE(plugin_id,customer_id,project_id,name,slot), UNIQUE(plugin_id,customer_id,project_id,idempotency_key),
 FOREIGN KEY(plugin_id,customer_id,project_id) REFERENCES webdock_auth.plugin_grant(plugin_id,customer_id,project_id),
 CHECK(status<>'running' OR (claim_token IS NOT NULL AND lease_until IS NOT NULL))
);
CREATE INDEX plugin_job_execution_claim ON webdock_auth.plugin_job_execution(available_at,id) WHERE status IN ('pending','running');
CREATE TABLE webdock_auth.plugin_invocation (
 id uuid PRIMARY KEY, plugin_id text NOT NULL REFERENCES webdock_auth.plugin_installation(id), customer_id text NOT NULL, project_id text NOT NULL,
 runtime_generation bigint NOT NULL CHECK(runtime_generation>0), grant_revision bigint NOT NULL CHECK(grant_revision>=0),
 kind text NOT NULL CHECK(kind IN ('action','job')), name text NOT NULL, intent text NOT NULL CHECK(intent IN ('read','write')),
 actor_id text, session_id text, job_claim_token uuid, job_execution_id text REFERENCES webdock_auth.plugin_job_execution(id),
 operation_id text NOT NULL, capabilities jsonb NOT NULL CHECK(jsonb_typeof(capabilities)='array'),
 created_at timestamptz NOT NULL DEFAULT now(), expires_at timestamptz NOT NULL, revoked_at timestamptz,
 CHECK(expires_at>created_at AND expires_at<=created_at+interval '60 seconds'),
 CHECK((kind='action' AND actor_id IS NOT NULL AND session_id IS NOT NULL AND job_execution_id IS NULL) OR
       (kind='job' AND actor_id IS NULL AND session_id IS NULL AND job_execution_id IS NOT NULL AND job_claim_token IS NOT NULL)),
 FOREIGN KEY(plugin_id,customer_id,project_id) REFERENCES webdock_auth.plugin_grant(plugin_id,customer_id,project_id)
);
CREATE TABLE webdock_auth.plugin_document_receipt (
 plugin_id text NOT NULL REFERENCES webdock_auth.plugin_installation(id), customer_id text NOT NULL, project_id text NOT NULL,
 operation_id text NOT NULL, idempotency_key text NOT NULL CHECK(length(idempotency_key) BETWEEN 1 AND 128),
 key text NOT NULL CHECK(length(key) BETWEEN 1 AND 240), payload_sha256 text NOT NULL CHECK(payload_sha256 ~ '^[a-f0-9]{64}$'),
 result jsonb NOT NULL CHECK(octet_length(result::text)<=65536), created_at timestamptz NOT NULL DEFAULT now(),
 PRIMARY KEY(plugin_id,customer_id,project_id,operation_id,idempotency_key)
);
DO $$ DECLARE t text; BEGIN
 FOREACH t IN ARRAY ARRAY['plugin_configuration','plugin_runtime','plugin_participant','plugin_job_execution','plugin_invocation','plugin_document_receipt'] LOOP
  EXECUTE format('ALTER TABLE webdock_auth.%I ENABLE ROW LEVEL SECURITY',t);
  EXECUTE format('ALTER TABLE webdock_auth.%I FORCE ROW LEVEL SECURITY',t);
  EXECUTE format('REVOKE ALL ON webdock_auth.%I FROM PUBLIC',t);
 END LOOP;
END $$;
