GRANT USAGE,CREATE ON SCHEMA webdock_rls TO webdock_platform_owner;
CREATE FUNCTION webdock_rls.billing_secret(env text,kind text) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE row webdock_auth.billing_connection;
BEGIN
 IF env NOT IN ('sandbox','live') OR kind NOT IN ('token','webhook') THEN RAISE EXCEPTION 'Invalid billing credential request' USING ERRCODE='42501';END IF;
 IF kind='webhook' THEN
  IF NOT webdock_rls.worker(ARRAY['polar:'||env||':inbox']) THEN RAISE EXCEPTION 'Inbox authority required' USING ERRCODE='42501';END IF;
 ELSE
  IF NOT webdock_rls.worker(ARRAY['polar:'||env||':catalog','polar:'||env||':customer','polar:'||env||':dispatcher']) THEN RAISE EXCEPTION 'Billing worker authority required' USING ERRCODE='42501';END IF;
 END IF;
 SELECT * INTO row FROM webdock_auth.billing_connection WHERE environment=env;
 IF NOT FOUND THEN RETURN NULL;END IF;
 IF kind='token' THEN RETURN jsonb_build_object('encrypted',row.encrypted_token,'version',row.token_version,'organizationID',row.organization_id);END IF;
 RETURN jsonb_build_object('encrypted',row.encrypted_webhook_secret,'version',row.webhook_version,'previous',CASE WHEN row.previous_webhook_until>now() THEN row.previous_webhook_secret END,'previousVersion',row.previous_webhook_version);
END $$;
ALTER FUNCTION webdock_rls.billing_secret(text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.billing_secret(text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.billing_secret(text,text) TO webdock_worker_runtime;
REVOKE CREATE ON SCHEMA webdock_rls FROM webdock_platform_owner;
