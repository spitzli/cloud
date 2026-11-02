GRANT USAGE,CREATE ON SCHEMA webdock_rls TO webdock_platform_owner;
CREATE FUNCTION webdock_rls.retry_billing_read(job_id text) RETURNS void LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE job webdock_auth.billing_job;event webdock_auth.billing_webhook_event;
BEGIN
 IF NOT webdock_rls.operator() THEN RAISE EXCEPTION 'Operator required' USING ERRCODE='42501';END IF;
 -- Keep receipt→job lock ordering consistent with delivery conflict quarantine.
 SELECT * INTO job FROM webdock_auth.billing_job WHERE id=job_id;
 IF NOT FOUND OR job.kind NOT IN ('webhook','reconcile','catalog') OR job.status NOT IN ('failed','needs_review') THEN RAISE EXCEPTION 'Read retry unavailable' USING ERRCODE='55000';END IF;
 IF job.event_id IS NOT NULL THEN
  SELECT * INTO event FROM webdock_auth.billing_webhook_event WHERE id=job.event_id FOR UPDATE;
  IF NOT FOUND OR event.conflict_hash IS NOT NULL OR event.api_version<>'2026-10' OR event.encrypted_body IS NULL THEN RAISE EXCEPTION 'Receipt requires review' USING ERRCODE='55000';END IF;
 END IF;
 SELECT * INTO job FROM webdock_auth.billing_job WHERE id=job_id FOR UPDATE;
 IF job.status NOT IN ('failed','needs_review') THEN RAISE EXCEPTION 'Read retry unavailable' USING ERRCODE='55000';END IF;
 IF job.event_id IS NOT NULL THEN UPDATE webdock_auth.billing_webhook_event SET status='received',processed_at=NULL,error_code=NULL WHERE id=job.event_id;END IF;
 UPDATE webdock_auth.billing_job SET status='pending',attempts=0,available_at=now(),claim_token=NULL,lease_until=NULL,error_code=NULL WHERE id=job_id;
END $$;
ALTER FUNCTION webdock_rls.retry_billing_read(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.retry_billing_read(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.retry_billing_read(text) TO webdock_auth_runtime;
REVOKE CREATE ON SCHEMA webdock_rls FROM webdock_platform_owner;
