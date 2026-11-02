GRANT USAGE,CREATE ON SCHEMA webdock_rls TO webdock_platform_owner;
CREATE FUNCTION webdock_rls.billing_checkout_job(env text,attempt text) RETURNS void LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE row webdock_auth.billing_checkout;
BEGIN
 SELECT * INTO row FROM webdock_auth.billing_checkout WHERE environment=env AND id=attempt AND state='creating';
 IF NOT FOUND OR NOT webdock_rls.billing_admin(row.customer_id) THEN RAISE EXCEPTION 'Authorized checkout required' USING ERRCODE='42501';END IF;
 INSERT INTO webdock_auth.billing_job(environment,customer_id,kind,dedupe_key,status,claim_token,lease_until)
 VALUES(env,row.customer_id,'checkout','checkout:'||row.id,'running',gen_random_uuid(),now()+interval '60 seconds') ON CONFLICT(environment,dedupe_key) DO NOTHING;
END $$;
ALTER FUNCTION webdock_rls.billing_checkout_job(text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.billing_checkout_job(text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.billing_checkout_job(text,text) TO webdock_auth_runtime;
REVOKE CREATE ON SCHEMA webdock_rls FROM webdock_platform_owner;
