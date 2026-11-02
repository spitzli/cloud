/** Server-only Mail bridges: return scoped metadata/aggregates, never grant the master table to tenants. */
export const databaseMailRlsSQL = String.raw`
CREATE OR REPLACE FUNCTION webdock_rls.mail_connection_status() RETURNS jsonb LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE result jsonb;
BEGIN
 IF webdock_rls.request_scope() IS NULL THEN RAISE EXCEPTION 'Mail authorization required' USING ERRCODE='42501'; END IF;
 SELECT jsonb_build_object('configured',true,'consumerKeySuffix',key_suffix) INTO result FROM webdock_auth.platform_mail_credential WHERE id;
 RETURN coalesce(result,'{"configured":false}'::jsonb);
END $$;
CREATE OR REPLACE FUNCTION webdock_rls.mail_provider_secret(customer text DEFAULT NULL,connection text DEFAULT NULL) RETURNS text LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE result text; permitted boolean;
BEGIN
 IF webdock_rls.request_scope()->>'kind'='mail-key' AND (connection IS NULL OR connection IS DISTINCT FROM webdock_rls.request_scope()->>'connectionID') THEN RAISE EXCEPTION 'Mail provider authorization required' USING ERRCODE='42501'; END IF;
 permitted:=webdock_rls.operator();
 IF NOT permitted AND customer IS NOT NULL THEN
  permitted:=(webdock_rls.customer_allowed(customer) AND webdock_rls.actor()->'preview'='null'::jsonb) OR webdock_rls.mail_key(customer,connection) OR webdock_rls.worker(ARRAY['mail-cron','mail-send-gateway']);
  permitted:=coalesce(permitted,false) AND EXISTS(SELECT 1 FROM webdock_auth.mail_tenant_account a WHERE a.customer_id=customer AND a.state='ready' AND a.provider_id IS NOT NULL AND (connection IS NULL OR EXISTS(SELECT 1 FROM webdock_auth.mail_connection c WHERE c.id=connection AND c.customer_id=customer AND c.kind='platform' AND c.provider_account_id=a.provider_id AND c.deleted_at IS NULL)));
 END IF;
 IF permitted IS NOT TRUE THEN RAISE EXCEPTION 'Mail provider authorization required' USING ERRCODE='42501'; END IF;
 SELECT encrypted_secret INTO result FROM webdock_auth.platform_mail_credential WHERE id;
 RETURN result;
END $$;
CREATE OR REPLACE FUNCTION webdock_rls.mail_usage_total(customer text,period date) RETURNS text LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE total numeric; native numeric:=0;
BEGIN
 IF (webdock_rls.customer_allowed(customer) OR webdock_rls.mail_key(customer) OR webdock_rls.worker(ARRAY['mail-cron','mail-send-gateway'])) IS NOT TRUE THEN RAISE EXCEPTION 'Mail quota authorization required' USING ERRCODE='42501'; END IF;
 SELECT coalesce(sum(reserved+consumed),0) INTO total FROM webdock_auth.mail_usage_period WHERE customer_id=customer AND mail_usage_period.period=mail_usage_total.period;
 IF to_regclass('webdock_mail.month_usage') IS NOT NULL THEN EXECUTE 'SELECT coalesce(sum(reserved+accepted),0) FROM webdock_mail.month_usage WHERE customer_id=$1 AND period_start=$2' INTO native USING customer,period; END IF;
 RETURN (total+native)::text;
END $$;
CREATE OR REPLACE FUNCTION webdock_rls.mail_admission_count(customer text,kind text) RETURNS bigint LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 IF (webdock_rls.customer_allowed(customer) OR webdock_rls.mail_key(customer) OR webdock_rls.worker(ARRAY['mail-cron','mail-send-gateway'])) IS NOT TRUE THEN RAISE EXCEPTION 'Mail admission authorization required' USING ERRCODE='42501'; END IF;
 IF kind='keys' THEN RETURN (SELECT count(*) FROM webdock_auth.mail_api_key WHERE customer_id=customer AND created_at>statement_timestamp()-interval '1 hour'); END IF;
 IF kind='scheduled' THEN RETURN (SELECT count(*) FROM webdock_auth.mail_message WHERE customer_id=customer AND dispatch_status='scheduled'); END IF;
 RAISE EXCEPTION 'Unknown Mail admission count' USING ERRCODE='22023';
END $$;
CREATE OR REPLACE FUNCTION webdock_rls.mail_refresh_domain(customer text,connection text,domain_id text,snapshot jsonb) RETURNS void LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 IF (webdock_rls.customer_allowed(customer) AND webdock_rls.actor()->'preview'='null'::jsonb OR webdock_rls.mail_key(customer,connection) OR webdock_rls.worker(ARRAY['mail-cron','mail-send-gateway'])) IS NOT TRUE THEN RAISE EXCEPTION 'Mail domain authorization required' USING ERRCODE='42501'; END IF;
 IF webdock_rls.request_scope()->>'kind'='mail-key' AND NOT EXISTS(SELECT 1 FROM webdock_auth.mail_api_key k WHERE k.id=webdock_rls.request_scope()->>'keyID' AND (k.domain_id IS NULL OR k.domain_id=mail_refresh_domain.domain_id)) THEN RAISE EXCEPTION 'Mail domain authorization required' USING ERRCODE='42501'; END IF;
 UPDATE webdock_auth.mail_connection_domain b SET provider_snapshot=snapshot,provider_checked_at=statement_timestamp()
 FROM webdock_auth.mail_sender_domain d,webdock_auth.mail_connection c WHERE b.customer_id=customer AND b.connection_id=connection AND b.domain_id=mail_refresh_domain.domain_id
 AND d.id=b.domain_id AND d.customer_id=customer AND d.deleted_at IS NULL AND d.status='ownership_verified'
 AND c.id=connection AND c.customer_id=customer AND c.deleted_at IS NULL
 AND b.provider_id=snapshot->>'id' AND d.domain=snapshot->>'domain';
 IF NOT FOUND THEN RAISE EXCEPTION 'Mail domain observation changed' USING ERRCODE='42501'; END IF;
END $$;
ALTER FUNCTION webdock_rls.mail_connection_status() OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.mail_provider_secret(text,text) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.mail_usage_total(text,date) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.mail_admission_count(text,text) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.mail_refresh_domain(text,text,text,jsonb) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.mail_connection_status(),webdock_rls.mail_provider_secret(text,text),webdock_rls.mail_usage_total(text,date),webdock_rls.mail_admission_count(text,text),webdock_rls.mail_refresh_domain(text,text,text,jsonb) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.mail_connection_status(),webdock_rls.mail_provider_secret(text,text),webdock_rls.mail_usage_total(text,date),webdock_rls.mail_admission_count(text,text),webdock_rls.mail_refresh_domain(text,text,text,jsonb) TO webdock_auth_runtime,webdock_worker_runtime;
`;
