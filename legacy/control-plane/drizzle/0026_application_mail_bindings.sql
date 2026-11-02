ALTER TABLE webdock_auth.hosting_service_binding ADD COLUMN IF NOT EXISTS mail_key_id text;
ALTER TABLE webdock_auth.hosting_service_binding ADD COLUMN IF NOT EXISTS retiring_mail_key_id text;
GRANT USAGE,CREATE ON SCHEMA webdock_rls TO webdock_platform_owner;
CREATE FUNCTION webdock_rls.binding_mail_ready(binding text) RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
BEGIN
 RETURN EXISTS(SELECT 1 FROM webdock_auth.hosting_service_binding b JOIN webdock_auth.mail_api_key k ON k.id=b.mail_key_id AND k.customer_id=b.customer_id JOIN webdock_auth.mail_connection c ON c.id=k.connection_id AND c.customer_id=b.customer_id JOIN webdock_auth.mail_sender_domain d ON d.id=k.domain_id AND d.customer_id=b.customer_id CROSS JOIN webdock_auth.platform_settings p WHERE b.id=binding AND k.revoked_at IS NULL AND c.state='ready' AND c.deleted_at IS NULL AND d.status='ownership_verified' AND d.deleted_at IS NULL AND p.id=true AND p.settings->>'mailGatewayEnabled'='true' AND (c.kind<>'platform' OR p.settings->>'mailEnabled'='true') AND (webdock_rls.customer_allowed(b.customer_id::text) OR webdock_rls.worker(ARRAY['git-worker']) OR webdock_rls.agent_project(b.project_id::text)));
END $$;
CREATE FUNCTION webdock_rls.revoke_binding_mail_key(binding text,retiring boolean) RETURNS void LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE b webdock_auth.hosting_service_binding;
BEGIN
 SELECT * INTO b FROM webdock_auth.hosting_service_binding WHERE id=binding FOR UPDATE;
 IF b.id IS NULL OR NOT (webdock_rls.customer_allowed(b.customer_id::text,true) OR webdock_rls.agent_project(b.project_id::text)) THEN RAISE EXCEPTION 'Binding ownership required' USING ERRCODE='42501'; END IF;
 UPDATE webdock_auth.mail_api_key SET revoked_at=coalesce(revoked_at,now()) WHERE customer_id=b.customer_id AND (id=b.retiring_mail_key_id OR (NOT retiring AND id=b.mail_key_id));
 UPDATE webdock_auth.hosting_service_binding SET retiring_mail_key_id=NULL WHERE id=binding;
END $$;
ALTER FUNCTION webdock_rls.binding_mail_ready(text) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.revoke_binding_mail_key(text,boolean) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.binding_mail_ready(text),webdock_rls.revoke_binding_mail_key(text,boolean) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.binding_mail_ready(text),webdock_rls.revoke_binding_mail_key(text,boolean) TO webdock_auth_runtime,webdock_worker_runtime;
REVOKE CREATE ON SCHEMA webdock_rls FROM webdock_platform_owner;
