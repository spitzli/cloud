-- Native0019. Tenant-only Mail grant; the caller must also prove its durable Mail job.
CREATE OR REPLACE FUNCTION webdock_rls.mail_plugin_policy(customer text, continuation boolean) RETURNS jsonb
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE binding webdock_mail.provider_binding%ROWTYPE; transport webdock_mail.send_transport%ROWTYPE;
 installation webdock_auth.plugin_installation%ROWTYPE; runtime webdock_auth.plugin_runtime%ROWTYPE;
 config webdock_auth.plugin_configuration%ROWTYPE; release webdock_auth.plugin_release%ROWTYPE;
 module_state text; selected_plugin text; manifest jsonb; action jsonb;
BEGIN
 IF NOT webdock_rls.worker(ARRAY['mail-send-gateway']) OR continuation IS NULL THEN RETURN NULL; END IF;
 PERFORM pg_advisory_xact_lock_shared(hashtext('webdock-plugin-installation'));
 -- Match invocation metadata order. Mail callers lock customer/transport before their job.
 PERFORM id FROM webdock_admin.customers WHERE id=customer AND status='active' FOR SHARE;
 IF NOT FOUND THEN RETURN NULL; END IF;
 SELECT * INTO transport FROM webdock_mail.send_transport WHERE customer_id=customer FOR SHARE;
 IF NOT FOUND OR transport.state<>'ready' THEN RETURN NULL; END IF;
 SELECT state INTO module_state FROM webdock_auth.plugin_installation WHERE id='mail' AND kind='builtin' FOR SHARE;
 IF module_state IS NULL OR (module_state<>'enabled' AND NOT(continuation AND module_state='draining')) THEN RETURN NULL; END IF;
 SELECT plugin_id INTO selected_plugin FROM webdock_mail.provider_binding WHERE customer_id=customer;
 IF selected_plugin IS NULL THEN RETURN NULL; END IF;
 SELECT * INTO installation FROM webdock_auth.plugin_installation WHERE id=selected_plugin AND kind='external' FOR SHARE;
 IF NOT FOUND OR installation.runtime<>'ready' OR (installation.state<>'enabled' AND NOT(continuation AND installation.state='draining')) THEN RETURN NULL; END IF;
 SELECT * INTO config FROM webdock_auth.plugin_configuration WHERE plugin_id=selected_plugin FOR SHARE;
 IF NOT FOUND THEN RETURN NULL; END IF;
 SELECT * INTO runtime FROM webdock_auth.plugin_runtime WHERE plugin_id=selected_plugin AND generation=installation.current_generation FOR SHARE;
 IF NOT FOUND OR runtime.configuration_revision IS DISTINCT FROM config.revision OR (runtime.status<>'ready' AND NOT(continuation AND runtime.status='draining')) THEN RETURN NULL; END IF;
 SELECT * INTO release FROM webdock_auth.plugin_release WHERE plugin_id=selected_plugin AND version=installation.version FOR SHARE;
 IF NOT FOUND OR release.image IS DISTINCT FROM runtime.image OR release.version IS DISTINCT FROM runtime.version
  OR release.manifest_sha256 IS DISTINCT FROM encode(sha256(convert_to(release.manifest_bytes,'UTF8')),'hex') THEN RETURN NULL; END IF;
 SELECT * INTO binding FROM webdock_mail.provider_binding WHERE customer_id=customer FOR SHARE;
 IF NOT FOUND OR binding.plugin_id IS DISTINCT FROM selected_plugin OR binding.revoked_at IS NOT NULL
  OR binding.transport_revision IS DISTINCT FROM transport.revision THEN RETURN NULL; END IF;
 manifest:=release.manifest_bytes::jsonb;
 IF manifest->>'name' IS DISTINCT FROM selected_plugin OR manifest->>'version' IS DISTINCT FROM installation.version
  OR manifest->>'image' IS DISTINCT FROM release.image THEN RETURN NULL; END IF;
 SELECT value INTO action FROM jsonb_array_elements(manifest->'actions') WHERE value->>'name'='mail.send';
 IF action IS NULL OR action->>'intent' IS DISTINCT FROM 'write' OR action ? 'httpBinding'
  OR jsonb_typeof(action->'capabilities') IS DISTINCT FROM 'array'
  OR NOT(config.approved_capabilities @> (action->'capabilities'))
  OR EXISTS(SELECT 1 FROM jsonb_array_elements_text(action->'capabilities') c WHERE c NOT IN ('plugin.http.read','plugin.http.request'))
  OR EXISTS(SELECT 1 FROM jsonb_array_elements(manifest->'services'->'required') s WHERE s->>'name'<>'http' OR s->>'version'<>'1')
 THEN RETURN NULL; END IF;
 RETURN jsonb_build_object('binding',to_jsonb(binding),'transport',jsonb_build_object('customer_id',transport.customer_id,'revision',transport.revision,'state',transport.state,'mode',transport.mode,'region',transport.region,'byok_revision',transport.byok_revision),
  'installation',to_jsonb(installation),'runtime',to_jsonb(runtime),'configuration',to_jsonb(config),'release',to_jsonb(release));
END $$;
ALTER FUNCTION webdock_rls.mail_plugin_policy(text,boolean) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.mail_plugin_policy(text,boolean) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.mail_plugin_policy(text,boolean) TO webdock_worker_runtime;
