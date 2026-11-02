-- Retained installation metadata; availability is independent of executable health.
CREATE TABLE webdock_auth.plugin_bootstrap (
 singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton), mode text NOT NULL CHECK(mode IN ('fresh','legacy')),
 initialized_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE webdock_auth.plugin_installation (
 id text PRIMARY KEY CHECK(id ~ '^[a-z][a-z0-9-]{0,63}$'), kind text NOT NULL CHECK(kind IN ('builtin','external')),
 revision bigint NOT NULL DEFAULT 0 CHECK(revision>=0), generation bigint NOT NULL DEFAULT 0 CHECK(generation>=0),
 desired_state text NOT NULL CHECK(desired_state IN ('enabled','draining','disabled')),
 state text NOT NULL CHECK(state IN ('enabled','draining','disabled')), version text NOT NULL,
 image_digest text, runtime text NOT NULL DEFAULT 'absent' CHECK(runtime IN ('absent','starting','ready','stopped','failed')),
 dependencies jsonb NOT NULL DEFAULT '[]' CHECK(jsonb_typeof(dependencies)='array'),
 blockers jsonb NOT NULL DEFAULT '[]' CHECK(jsonb_typeof(blockers)='array'),
 updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE webdock_auth.plugin_operation (
 id text PRIMARY KEY DEFAULT webdock_auth.next_snowflake(), plugin_id text NOT NULL REFERENCES webdock_auth.plugin_installation(id),
 actor_id text NOT NULL, idempotency_key text NOT NULL CHECK(length(idempotency_key) BETWEEN 1 AND 128),
 payload jsonb NOT NULL, status text NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','running','succeeded','failed','blocked')),
 generation bigint NOT NULL, phase text NOT NULL DEFAULT 'requested', checkpoint jsonb NOT NULL DEFAULT '{}',
 claim_token uuid, lease_until timestamptz, attempts integer NOT NULL DEFAULT 0,
 available_at timestamptz NOT NULL DEFAULT now(), created_at timestamptz NOT NULL DEFAULT now(),
 updated_at timestamptz NOT NULL DEFAULT now(), UNIQUE(actor_id,idempotency_key)
);
CREATE INDEX plugin_operation_claim ON webdock_auth.plugin_operation(available_at,id) WHERE status IN ('pending','running');
CREATE TABLE webdock_auth.plugin_release (
 plugin_id text NOT NULL REFERENCES webdock_auth.plugin_installation(id), version text NOT NULL,
 manifest_bytes text NOT NULL CHECK(octet_length(manifest_bytes)<=262144), manifest_sha256 text NOT NULL CHECK(manifest_sha256 ~ '^[a-f0-9]{64}$'),
 image text NOT NULL, registry_url text NOT NULL, signer_key_id text NOT NULL, created_at timestamptz NOT NULL DEFAULT now(), PRIMARY KEY(plugin_id,version)
);
CREATE TABLE webdock_auth.plugin_grant (
 plugin_id text NOT NULL REFERENCES webdock_auth.plugin_installation(id), customer_id text NOT NULL, project_id text NOT NULL,
 revision bigint NOT NULL DEFAULT 0 CHECK(revision>=0), capabilities jsonb NOT NULL DEFAULT '[]', jobs jsonb NOT NULL DEFAULT '[]',
 revoked_at timestamptz, PRIMARY KEY(plugin_id,customer_id,project_id)
);
CREATE TABLE webdock_auth.plugin_document (
 plugin_id text NOT NULL REFERENCES webdock_auth.plugin_installation(id), customer_id text NOT NULL, project_id text NOT NULL,
 key text NOT NULL CHECK(length(key) BETWEEN 1 AND 240), value jsonb NOT NULL CHECK(octet_length(value::text)<=65536),
 revision bigint NOT NULL DEFAULT 0 CHECK(revision>=0), updated_at timestamptz NOT NULL DEFAULT now(), PRIMARY KEY(plugin_id,customer_id,project_id,key)
);
-- Runtime policy installation classifies these tables. Nothing is public before it runs.
ALTER TABLE webdock_auth.plugin_bootstrap ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.plugin_bootstrap FORCE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.plugin_installation ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.plugin_installation FORCE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.plugin_operation ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.plugin_operation FORCE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.plugin_release ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.plugin_release FORCE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.plugin_grant ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.plugin_grant FORCE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.plugin_document ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.plugin_document FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_auth.plugin_bootstrap,webdock_auth.plugin_installation,webdock_auth.plugin_operation,webdock_auth.plugin_release,webdock_auth.plugin_grant,webdock_auth.plugin_document FROM PUBLIC;
