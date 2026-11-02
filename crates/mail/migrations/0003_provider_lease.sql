-- One provider attempt per already admitted send job, independent of project/session Invoke.
CREATE TABLE webdock_mail.provider_lease (
 id uuid PRIMARY KEY,
 job_id text NOT NULL UNIQUE REFERENCES webdock_mail.send_job(id),
 customer_id text NOT NULL REFERENCES webdock_admin.customers(id),
 plugin_id text NOT NULL REFERENCES webdock_auth.plugin_installation(id),
 queue_lease_token uuid NOT NULL,
 token_sha256 text NOT NULL CHECK(token_sha256 ~ '^[a-f0-9]{64}$'),
 claims_json text NOT NULL CHECK(octet_length(claims_json)<=16384),
 issued_at timestamptz NOT NULL,
 expires_at timestamptz NOT NULL,
 attempted_at timestamptz,
 revoked_at timestamptz,
 CHECK(expires_at>issued_at AND expires_at<=issued_at+interval '60 seconds'),
 CHECK(attempted_at IS NULL OR attempted_at BETWEEN issued_at AND expires_at),
 CHECK(coalesce(
  jsonb_typeof(claims_json::jsonb)='object'
  AND claims_json::jsonb->>'v'='1'
  AND claims_json::jsonb->>'purpose'='mail-send'
  AND claims_json::jsonb->>'audience'='webdock.mail.runtime.v1'
  AND claims_json::jsonb->>'id'=id::text
  AND claims_json::jsonb->>'jobID'=job_id
  AND claims_json::jsonb->>'customerID'=customer_id
  AND claims_json::jsonb->>'pluginID'=plugin_id
  AND (claims_json::jsonb->>'iat')::bigint=extract(epoch FROM issued_at)
  AND (claims_json::jsonb->>'exp')::bigint=extract(epoch FROM expires_at),false))
);
ALTER TABLE webdock_mail.provider_lease OWNER TO webdock_platform_owner;
ALTER TABLE webdock_mail.provider_lease ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_mail.provider_lease FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_mail.provider_lease FROM PUBLIC;
GRANT SELECT,INSERT,UPDATE ON webdock_mail.provider_lease TO webdock_worker_runtime;
CREATE POLICY platform_owner ON webdock_mail.provider_lease TO webdock_platform_owner USING(true) WITH CHECK(true);
CREATE POLICY native_mail_lease_read ON webdock_mail.provider_lease FOR SELECT TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['mail-send-gateway']));
CREATE POLICY native_mail_lease_create ON webdock_mail.provider_lease FOR INSERT TO webdock_worker_runtime WITH CHECK(webdock_rls.worker(ARRAY['mail-send-gateway']) AND attempted_at IS NULL AND revoked_at IS NULL
 AND EXISTS(SELECT 1 FROM webdock_mail.send_job j WHERE j.id=provider_lease.job_id AND j.customer_id=provider_lease.customer_id AND j.lease_token=provider_lease.queue_lease_token AND j.state='handoff' AND j.route->>'kind'='direct' AND j.lease_until>clock_timestamp()));
CREATE POLICY native_mail_lease_attempt ON webdock_mail.provider_lease FOR UPDATE TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['mail-send-gateway'])) WITH CHECK(webdock_rls.worker(ARRAY['mail-send-gateway']));
CREATE FUNCTION webdock_mail.preserve_provider_attempt() RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 IF (to_jsonb(NEW)-ARRAY['attempted_at','revoked_at']) IS DISTINCT FROM (to_jsonb(OLD)-ARRAY['attempted_at','revoked_at'])
  OR (OLD.attempted_at IS NOT NULL AND NEW.attempted_at IS DISTINCT FROM OLD.attempted_at)
  OR (OLD.revoked_at IS NOT NULL AND NEW.revoked_at IS DISTINCT FROM OLD.revoked_at) THEN
  RAISE EXCEPTION 'Mail provider lease and attempt are immutable' USING ERRCODE='23514';
 END IF;
 IF OLD.attempted_at IS NULL AND NEW.attempted_at IS NOT NULL THEN
  IF OLD.revoked_at IS NOT NULL OR OLD.expires_at<=clock_timestamp()
   OR NOT EXISTS(SELECT 1 FROM webdock_mail.send_job j WHERE j.id=OLD.job_id AND j.customer_id=OLD.customer_id AND j.lease_token=OLD.queue_lease_token AND j.state='handoff' AND j.route->>'kind'='direct' AND j.lease_until>clock_timestamp()) THEN
   RAISE EXCEPTION 'Current Mail handoff required' USING ERRCODE='23514';
  END IF;
  NEW.attempted_at:=clock_timestamp();
 END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_mail.preserve_provider_attempt() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_mail.preserve_provider_attempt() FROM PUBLIC;
CREATE TRIGGER provider_lease_immutable BEFORE UPDATE ON webdock_mail.provider_lease FOR EACH ROW EXECUTE FUNCTION webdock_mail.preserve_provider_attempt();
