-- Explicit offline enrollment; Core cannot create/replace its own transport authority.
ALTER TABLE webdock_auth.hosting_cluster ADD CONSTRAINT hosting_cluster_installation_identity UNIQUE(id,runtime_installation);
CREATE TABLE IF NOT EXISTS webdock_auth.agent_transport_enrollment (
 installation text NOT NULL CHECK(installation ~ '^[a-z][a-z0-9-]{0,23}$'),
 agent_id text NOT NULL CHECK(agent_id ~ '^[a-z][a-z0-9-]{0,63}$'),
 generation integer NOT NULL CHECK(generation>0),
 cluster_id varchar NOT NULL,
 credential_hash text NOT NULL CHECK(credential_hash ~ '^[0-9a-f]{64}$'),
 transport text NOT NULL CHECK(transport IN ('unix','mtls')),
 unix_uid bigint CHECK(unix_uid BETWEEN 0 AND 4294967295),
 certificate_sha256 text CHECK(certificate_sha256 ~ '^[0-9a-f]{64}$'),
 revoked boolean NOT NULL DEFAULT false,
 last_sequence bigint NOT NULL DEFAULT 0 CHECK(last_sequence>=0),
 last_seen timestamptz,
 PRIMARY KEY(installation,agent_id),
 FOREIGN KEY(cluster_id,installation) REFERENCES webdock_auth.hosting_cluster(id,runtime_installation) ON DELETE RESTRICT,
 CHECK((transport='unix' AND unix_uid IS NOT NULL AND certificate_sha256 IS NULL)
    OR (transport='mtls' AND unix_uid IS NULL AND certificate_sha256 IS NOT NULL))
);
CREATE UNIQUE INDEX IF NOT EXISTS agent_transport_certificate ON webdock_auth.agent_transport_enrollment(certificate_sha256) WHERE certificate_sha256 IS NOT NULL AND NOT revoked;
CREATE TABLE IF NOT EXISTS webdock_auth.agent_runtime_dispatch (
 operation_id text NOT NULL REFERENCES webdock_auth.hosting_operation(id) ON DELETE CASCADE,
 generation bigint NOT NULL CHECK(generation>0),
 installation text NOT NULL,
 agent_id text NOT NULL,
 agent_generation integer NOT NULL,
 request_sha256 text NOT NULL CHECK(request_sha256 ~ '^[0-9a-f]{64}$'),
 command jsonb NOT NULL CHECK(jsonb_typeof(command)='object'),
 receipt_sha256 text CHECK(receipt_sha256 ~ '^[0-9a-f]{64}$'),
 completed_at timestamptz,
 created_at timestamptz NOT NULL DEFAULT now(),
 PRIMARY KEY(operation_id,generation),
 FOREIGN KEY(installation,agent_id) REFERENCES webdock_auth.agent_transport_enrollment(installation,agent_id) ON DELETE RESTRICT
);
ALTER TABLE webdock_auth.agent_transport_enrollment OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.agent_runtime_dispatch OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.agent_transport_enrollment ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.agent_transport_enrollment FORCE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.agent_runtime_dispatch ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.agent_runtime_dispatch FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_auth.agent_transport_enrollment,webdock_auth.agent_runtime_dispatch FROM PUBLIC;
GRANT SELECT,UPDATE(last_sequence,last_seen) ON webdock_auth.agent_transport_enrollment TO webdock_auth_bootstrap;
GRANT SELECT,INSERT,UPDATE ON webdock_auth.agent_runtime_dispatch TO webdock_auth_bootstrap;
CREATE POLICY transport_owner ON webdock_auth.agent_transport_enrollment TO webdock_platform_owner USING(true) WITH CHECK(true);
CREATE POLICY transport_runtime ON webdock_auth.agent_transport_enrollment TO webdock_auth_bootstrap USING(true) WITH CHECK(true);
CREATE POLICY dispatch_owner ON webdock_auth.agent_runtime_dispatch TO webdock_platform_owner USING(true) WITH CHECK(true);
CREATE POLICY dispatch_runtime ON webdock_auth.agent_runtime_dispatch TO webdock_auth_bootstrap USING(true) WITH CHECK(true);
