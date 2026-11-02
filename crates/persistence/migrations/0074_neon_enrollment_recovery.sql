-- An explicit owner may repair an orphaned first enrollment, never through HTTP login.
CREATE TABLE webdock_auth.neon_enrollment_recovery (
 user_id text PRIMARY KEY REFERENCES webdock_auth.neon_identity(user_id) ON DELETE RESTRICT,
 provider_origin text NOT NULL,
 provider_subject uuid NOT NULL UNIQUE,
 snapshot_sha256 text NOT NULL CHECK(snapshot_sha256 ~ '^[a-f0-9]{64}$'),
 mapping_sha256 text NOT NULL CHECK(mapping_sha256 ~ '^[a-f0-9]{64}$'),
 evidence_ref text NOT NULL CHECK(char_length(evidence_ref) BETWEEN 1 AND 1024),
 evidence_sha256 text NOT NULL CHECK(evidence_sha256 ~ '^[a-f0-9]{64}$'),
 reviewer_ref text NOT NULL CHECK(char_length(reviewer_ref) BETWEEN 1 AND 1024),
 owner_role text NOT NULL CHECK(owner_role='neondb_owner'),
 created_at timestamptz NOT NULL DEFAULT statement_timestamp()
);
ALTER TABLE webdock_auth.neon_enrollment_recovery OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.neon_enrollment_recovery ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.neon_enrollment_recovery FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_auth.neon_enrollment_recovery FROM PUBLIC,webdock_auth_bootstrap,webdock_auth_runtime,webdock_registry_runtime,webdock_worker_runtime;
CREATE POLICY platform_owner ON webdock_auth.neon_enrollment_recovery TO webdock_platform_owner USING(true) WITH CHECK(true);
DO $$ BEGIN
 IF EXISTS(SELECT FROM pg_roles WHERE rolname='neondb_owner') THEN
  GRANT SELECT,INSERT ON webdock_auth.neon_enrollment_recovery TO neondb_owner;
  CREATE POLICY offline_owner ON webdock_auth.neon_enrollment_recovery TO neondb_owner USING(true) WITH CHECK(true);
 END IF;
END $$;
CREATE FUNCTION webdock_auth.neon_enrollment_recovery_immutable() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,pg_temp AS $$ BEGIN
 RAISE EXCEPTION 'Enrollment recovery audit is immutable' USING ERRCODE='23514';
END $$;
ALTER FUNCTION webdock_auth.neon_enrollment_recovery_immutable() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.neon_enrollment_recovery_immutable() FROM PUBLIC;
CREATE TRIGGER neon_enrollment_recovery_immutable BEFORE UPDATE OR DELETE ON webdock_auth.neon_enrollment_recovery
FOR EACH ROW EXECUTE FUNCTION webdock_auth.neon_enrollment_recovery_immutable();
CREATE TRIGGER neon_enrollment_recovery_no_truncate BEFORE TRUNCATE ON webdock_auth.neon_enrollment_recovery
FOR EACH STATEMENT EXECUTE FUNCTION webdock_auth.neon_enrollment_recovery_immutable();
