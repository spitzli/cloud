CREATE TABLE webdock_auth.plugin_source_import (
 id text PRIMARY KEY DEFAULT webdock_auth.next_snowflake(), actor_id text NOT NULL,
 actor_scope jsonb NOT NULL CHECK(octet_length(actor_scope::text)<=32768),
 project_id text NOT NULL, customer_id text NOT NULL,
 source_kind text NOT NULL CHECK(source_kind IN ('zip','git','github')), source_label text NOT NULL CHECK(length(source_label)<=2048),
 source_url text, source_ref text, source_commit text CHECK(source_commit ~ '^[a-f0-9]{40}$'), source_sha256 text CHECK(source_sha256 ~ '^[a-f0-9]{64}$'),
 secret_cipher text, runtime_cipher text, cluster_id text,
 expected_bytes integer NOT NULL DEFAULT 0 CHECK(expected_bytes BETWEEN 0 AND 33554432),
 uploaded_bytes integer NOT NULL DEFAULT 0 CHECK(uploaded_bytes BETWEEN 0 AND expected_bytes),
 mode text NOT NULL DEFAULT 'build' CHECK(mode='build'),
 status text NOT NULL CHECK(status IN ('uploading','queued','building','review','approved','deploying','ready','enabled','failed')),
 idempotency_key text NOT NULL CHECK(length(idempotency_key) BETWEEN 1 AND 128), request_sha256 text NOT NULL CHECK(request_sha256 ~ '^[a-f0-9]{64}$'),
 plugin_id text, manifest_raw text CHECK(octet_length(manifest_raw)<=262144), manifest_sha256 text CHECK(manifest_sha256 ~ '^[a-f0-9]{64}$'),
 configuration jsonb NOT NULL DEFAULT '{}' CHECK(octet_length(configuration::text)<=65536), approved_capabilities jsonb NOT NULL DEFAULT '[]',
 claim_token uuid, lease_until timestamptz, error_code text,
 created_at timestamptz NOT NULL DEFAULT now(), updated_at timestamptz NOT NULL DEFAULT now(), expires_at timestamptz NOT NULL DEFAULT now()+interval '24 hours',
 UNIQUE(actor_id,idempotency_key), CHECK(status NOT IN ('building','deploying') OR (claim_token IS NOT NULL AND lease_until IS NOT NULL))
);
CREATE INDEX plugin_source_claim ON webdock_auth.plugin_source_import(created_at,id) WHERE status IN ('queued','approved');
CREATE UNIQUE INDEX plugin_source_approved_plugin ON webdock_auth.plugin_source_import(plugin_id) WHERE status IN ('approved','deploying','ready');
CREATE TABLE webdock_auth.plugin_source_chunk (
 import_id text NOT NULL REFERENCES webdock_auth.plugin_source_import(id) ON DELETE CASCADE,
 offset_bytes integer NOT NULL CHECK(offset_bytes BETWEEN 0 AND 33554431), data bytea NOT NULL CHECK(octet_length(data) BETWEEN 1 AND 65536),
 PRIMARY KEY(import_id,offset_bytes)
);
ALTER TABLE webdock_auth.plugin_source_import ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.plugin_source_import FORCE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.plugin_source_chunk ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.plugin_source_chunk FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_auth.plugin_source_import,webdock_auth.plugin_source_chunk FROM PUBLIC;
