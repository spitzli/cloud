CREATE TABLE IF NOT EXISTS webdock_auth.clerk_identity_mapping (
 issuer text NOT NULL, external_subject text NOT NULL,
 local_subject text NOT NULL REFERENCES webdock_auth."user"(id) ON DELETE CASCADE,
 revoked_at timestamptz, created_at timestamptz NOT NULL DEFAULT now(),
 PRIMARY KEY(issuer,external_subject), UNIQUE(issuer,local_subject)
);
CREATE TABLE IF NOT EXISTS webdock_auth.clerk_session_mapping (
 issuer text NOT NULL, external_session text NOT NULL, external_subject text NOT NULL,
 local_session_id text NOT NULL UNIQUE REFERENCES webdock_auth.session(id) ON DELETE CASCADE,
 expires_at timestamptz NOT NULL,
 PRIMARY KEY(issuer,external_session),
 FOREIGN KEY(issuer,external_subject) REFERENCES webdock_auth.clerk_identity_mapping(issuer,external_subject) ON DELETE CASCADE
);
CREATE OR REPLACE FUNCTION webdock_auth.guard_clerk_identity_binding() RETURNS trigger
 LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 IF ROW(NEW.issuer,NEW.external_subject,NEW.local_subject) IS DISTINCT FROM ROW(OLD.issuer,OLD.external_subject,OLD.local_subject)
 OR (OLD.revoked_at IS NOT NULL AND NEW.revoked_at IS NULL) THEN
  RAISE EXCEPTION 'Clerk identity bindings cannot be reassigned or revived' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END $$;
REVOKE ALL ON FUNCTION webdock_auth.guard_clerk_identity_binding() FROM PUBLIC;
DROP TRIGGER IF EXISTS guard_clerk_identity_binding ON webdock_auth.clerk_identity_mapping;
CREATE TRIGGER guard_clerk_identity_binding BEFORE UPDATE ON webdock_auth.clerk_identity_mapping
 FOR EACH ROW EXECUTE FUNCTION webdock_auth.guard_clerk_identity_binding();
CREATE OR REPLACE FUNCTION webdock_auth.clamp_clerk_session_expiry() RETURNS trigger
 LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE lease timestamptz;
BEGIN
 SELECT expires_at INTO lease FROM webdock_auth.clerk_session_mapping WHERE local_session_id=NEW.id;
 IF FOUND THEN NEW."expiresAt" := least(NEW."expiresAt",lease); END IF;
 RETURN NEW;
END $$;
REVOKE ALL ON FUNCTION webdock_auth.clamp_clerk_session_expiry() FROM PUBLIC;
DROP TRIGGER IF EXISTS clamp_clerk_session_expiry ON webdock_auth.session;
CREATE TRIGGER clamp_clerk_session_expiry BEFORE UPDATE OF "expiresAt" ON webdock_auth.session
 FOR EACH ROW EXECUTE FUNCTION webdock_auth.clamp_clerk_session_expiry();
