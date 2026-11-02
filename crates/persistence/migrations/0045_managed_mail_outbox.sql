-- Native0045. Fixed native managed Mail control; retained migrations remain immutable.
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
 SELECT state INTO module_state FROM webdock_auth.plugin_installation WHERE id='mail' FOR SHARE;
 IF module_state IS NULL OR (module_state<>'enabled' AND NOT(continuation AND module_state='draining')) THEN RETURN NULL; END IF;
 SELECT plugin_id INTO selected_plugin FROM webdock_mail.provider_binding WHERE customer_id=customer;
 IF selected_plugin IS NULL THEN RETURN NULL; END IF;
 IF selected_plugin<>'mail' AND NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_installation WHERE id='mail' AND kind='builtin') THEN RETURN NULL; END IF;
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
  OR EXISTS(SELECT 1 FROM jsonb_array_elements(manifest->'services'->'required') s WHERE (s->>'name'<>'http' AND NOT(selected_plugin='mail' AND s->>'name'='native-mail-control')) OR s->>'version'<>'1')
 THEN RETURN NULL; END IF;
 RETURN jsonb_build_object('managedEnrollment',selected_plugin='mail' AND EXISTS(SELECT 1 FROM webdock_auth.plugin_install_execution WHERE plugin_id='mail'),'enrollment',(SELECT e.binding FROM webdock_auth.plugin_install_execution e WHERE e.plugin_id=selected_plugin AND e.runtime_generation=runtime.generation),'binding',to_jsonb(binding),'transport',jsonb_build_object('customer_id',transport.customer_id,'revision',transport.revision,'state',transport.state,'mode',transport.mode,'region',transport.region,'byok_revision',transport.byok_revision,'provider_account_id',transport.provider_account_id,'credentials_sha256',encode(sha256(convert_to(coalesce(transport.encrypted_credentials,''),'UTF8')),'hex')),
  'installation',to_jsonb(installation),'runtime',to_jsonb(runtime),'configuration',to_jsonb(config),'release',to_jsonb(release));
END $$;
ALTER FUNCTION webdock_rls.mail_plugin_policy(text,boolean) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.mail_plugin_policy(text,boolean) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.mail_plugin_policy(text,boolean) TO webdock_worker_runtime;

-- Read enrolled identity without opening a second worker connection. Disabled is bookkeeping only.
CREATE FUNCTION webdock_rls.managed_mail_runtime(generation bigint, bookkeeping boolean) RETURNS jsonb
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE p webdock_auth.plugin_installation%ROWTYPE; r webdock_auth.plugin_runtime%ROWTYPE;
 c webdock_auth.plugin_configuration%ROWTYPE; l webdock_auth.plugin_release%ROWTYPE; b jsonb;
BEGIN
 IF NOT webdock_rls.worker(ARRAY['mail-send-gateway']) OR generation<=0 OR bookkeeping IS NULL THEN RETURN NULL; END IF;
 PERFORM pg_advisory_xact_lock_shared(hashtext('webdock-plugin-installation'));
 SELECT * INTO p FROM webdock_auth.plugin_installation WHERE id='mail' AND kind='external' FOR SHARE;
 IF NOT FOUND OR p.current_generation IS DISTINCT FROM generation OR p.runtime<>'ready'
  OR NOT(p.state IN ('enabled','draining') OR bookkeeping AND p.state='disabled') THEN RETURN NULL; END IF;
 SELECT * INTO c FROM webdock_auth.plugin_configuration WHERE plugin_id='mail' FOR SHARE;
 IF NOT FOUND THEN RETURN NULL; END IF;
 SELECT * INTO r FROM webdock_auth.plugin_runtime WHERE plugin_id='mail' AND plugin_runtime.generation=p.current_generation FOR SHARE;
 IF NOT FOUND OR r.configuration_revision IS DISTINCT FROM c.revision OR r.status NOT IN ('ready','draining') THEN RETURN NULL; END IF;
 SELECT * INTO l FROM webdock_auth.plugin_release WHERE plugin_id='mail' AND version=p.version FOR SHARE;
 IF NOT FOUND OR l.image IS DISTINCT FROM r.image OR l.version IS DISTINCT FROM r.version
  OR l.manifest_sha256 IS DISTINCT FROM encode(sha256(convert_to(l.manifest_bytes,'UTF8')),'hex') THEN RETURN NULL; END IF;
 SELECT binding INTO b FROM webdock_auth.plugin_install_execution WHERE plugin_id='mail' AND runtime_generation=p.current_generation FOR SHARE;
 RETURN jsonb_build_object('installation',to_jsonb(p),'runtime',to_jsonb(r),'configuration',to_jsonb(c),'release',to_jsonb(l),'enrollment',b,
  'managedEnrollment',EXISTS(SELECT 1 FROM webdock_auth.plugin_install_execution WHERE plugin_id='mail'));
END $$;
ALTER FUNCTION webdock_rls.managed_mail_runtime(bigint,boolean) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.managed_mail_runtime(bigint,boolean) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.managed_mail_runtime(bigint,boolean) TO webdock_worker_runtime;

-- Nonsecret durable action lineage. No browser session/token expiry participates in this predicate.
CREATE FUNCTION webdock_rls.managed_mail_source(proof jsonb) RETURNS boolean
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE p webdock_auth.plugin_installation%ROWTYPE; g webdock_auth.plugin_grant%ROWTYPE;
 u webdock_auth."user"%ROWTYPE; a jsonb; l jsonb; client jsonb; consent jsonb; op boolean;
BEGIN
 IF NOT webdock_rls.worker(ARRAY['mail-send-gateway']) OR proof->>'kind' IS DISTINCT FROM 'action' OR proof->>'action' IS DISTINCT FROM 'mail.send' THEN RETURN false; END IF;
 PERFORM pg_advisory_xact_lock_shared(hashtext('webdock-plugin-installation'));
 SELECT * INTO p FROM webdock_auth.plugin_installation WHERE id='mail' AND kind='external' FOR SHARE;
 IF NOT FOUND OR p.state NOT IN ('enabled','draining') OR p.policy_revision::text IS DISTINCT FROM proof->>'policyRevision' THEN RETURN false; END IF;
 PERFORM c.id FROM webdock_admin.customers c JOIN webdock_admin.projects j ON j.customer_id=c.id
  WHERE c.id=proof->>'customerID' AND j.id=proof->>'projectID' AND c.status='active' AND j.status='active'
  AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=j.id AND d.status<>'deleted') FOR SHARE OF c,j;
 IF NOT FOUND THEN RETURN false; END IF;
 SELECT * INTO u FROM webdock_auth."user" WHERE id=proof->>'actorID' AND "emailVerified" AND NOT "mustChangePassword" AND NOT coalesce(banned,false) AND role IN ('user','operator') AND (role<>'operator' OR coalesce("twoFactorEnabled",false)) FOR SHARE;
 IF NOT FOUND THEN RETURN false; END IF;
 op:=u.role='operator' AND coalesce(u."twoFactorEnabled",false);
 IF proof->>'actorKind'='operator' THEN
  IF NOT op THEN RETURN false; END IF;
 ELSE
  PERFORM m.id FROM webdock_auth.member m JOIN webdock_auth.tenant_customer t ON t.organization_id=m."organizationId"
   WHERE m."userId"=u.id AND t.customer_id=proof->>'customerID' AND m.role IN ('owner','admin') AND m.id=proof->>'memberID' FOR SHARE OF m,t;
  IF NOT FOUND THEN RETURN false; END IF;
 END IF;
 SELECT * INTO g FROM webdock_auth.plugin_grant WHERE plugin_id='mail' AND project_id=proof->>'projectID' FOR SHARE;
 IF NOT FOUND OR g.customer_id IS DISTINCT FROM proof->>'customerID' OR g.revoked_at IS NOT NULL OR g.revision::text IS DISTINCT FROM proof->>'grantRevision' THEN RETURN false; END IF;
 SELECT x INTO a FROM webdock_auth.plugin_release r, LATERAL jsonb_array_elements(r.manifest_bytes::jsonb->'actions') x
  WHERE r.plugin_id='mail' AND r.version=p.version AND x->>'name'='mail.send';
 IF a IS NULL OR a->>'intent'<>'write' OR NOT(g.capabilities @> (a->'capabilities')) THEN RETURN false; END IF;
 IF a->>'feature' IS NOT NULL AND (NOT webdock_rls.plugin_feature_supported('mail',a->>'feature') OR NOT op AND NOT EXISTS(
  SELECT 1 FROM webdock_auth.plugin_feature_policy f WHERE f.plugin_id='mail' AND f.feature_id=a->>'feature'
  AND (f.mode='everyone' OR f.mode='selected' AND EXISTS(SELECT 1 FROM webdock_auth.plugin_feature_customer c WHERE c.plugin_id='mail' AND c.feature_id=f.feature_id AND c.customer_id=g.customer_id)))) THEN RETURN false; END IF;
 PERFORM resource_id FROM webdock_auth.plugin_resource WHERE plugin_id='mail' AND resource_id=proof->>'resourceID' AND offering_id='domain'
  AND project_id=g.project_id AND customer_id=g.customer_id AND name=proof->>'domain' FOR SHARE;
 IF NOT FOUND THEN RETURN false; END IF;
 l:=proof->'authorityLineage';
 IF l->>'source'='mcp' THEN
  -- Client/resource/consent grants are durable; access tokens, sessions and logout are not.
  IF l->>'writeScope' IS NULL OR l->>'writeScope' NOT IN ('webdock:write','hosting:write') THEN RETURN false; END IF;
  FOR client IN SELECT to_jsonb(c) FROM webdock_auth."oauthClient" c WHERE c."clientId" IN (l->>'callerID',l->>'issuerClientID') ORDER BY c."clientId" FOR SHARE LOOP
   IF coalesce((client->>'disabled')::boolean,false) OR coalesce((client->>'dpopBoundAccessTokens')::boolean,false)
    OR (client->'scopes' ? (l->>'writeScope')) IS DISTINCT FROM true
    OR client->>'updatedAt' IS DISTINCT FROM (CASE WHEN client->>'clientId'=l->>'callerID' THEN l->>'callerRevision' ELSE l->>'issuerRevision' END) THEN RETURN false; END IF;
  END LOOP;
  IF NOT EXISTS(SELECT 1 FROM webdock_auth."oauthClient" WHERE "clientId"=l->>'callerID') OR NOT EXISTS(SELECT 1 FROM webdock_auth."oauthClient" WHERE "clientId"=l->>'issuerClientID') THEN RETURN false; END IF;
  PERFORM identifier FROM webdock_auth."oauthResource" WHERE identifier=l->>'resource' AND NOT coalesce(disabled,false) AND NOT coalesce("dpopBoundAccessTokensRequired",false) AND "allowedScopes" ? (l->>'writeScope') FOR SHARE;
  IF NOT FOUND THEN RETURN false; END IF;
  PERFORM id FROM webdock_auth."oauthClientResource" WHERE id=l->>'callerResourceGrantID' AND "clientId"=l->>'callerID' AND "resourceId"=l->>'resource' FOR SHARE;
  IF NOT FOUND THEN RETURN false; END IF;
  PERFORM id FROM webdock_auth."oauthClientResource" WHERE id=l->>'issuerResourceGrantID' AND "clientId"=l->>'issuerClientID' AND "resourceId"=l->>'resource' FOR SHARE;
  IF NOT FOUND THEN RETURN false; END IF;
  IF l->>'skipConsent'='true' THEN
   IF NOT EXISTS(SELECT 1 FROM webdock_auth."oauthClient" WHERE "clientId"=l->>'issuerClientID' AND "skipConsent") THEN RETURN false; END IF;
  ELSE
   SELECT to_jsonb(c) INTO consent FROM webdock_auth."oauthConsent" c WHERE c.id=l->>'consentID' AND c."clientId"=l->>'issuerClientID' AND c."userId"=u.id FOR SHARE;
   IF consent IS NULL OR (consent->'scopes' ? (l->>'writeScope')) IS DISTINCT FROM true OR (consent->'resources' ? (l->>'resource')) IS DISTINCT FROM true OR consent->>'updatedAt' IS DISTINCT FROM l->>'consentRevision' THEN RETURN false; END IF;
  END IF;
 ELSIF l->>'source'='browser' THEN
  IF l ? 'clientID' THEN
   SELECT to_jsonb(c) INTO client FROM webdock_auth."oauthClient" c WHERE c."clientId"=l->>'clientID' AND NOT coalesce(c.disabled,false) AND NOT coalesce(c."dpopBoundAccessTokens",false) FOR SHARE;
   IF client IS NULL OR client->>'updatedAt' IS DISTINCT FROM l->>'clientRevision' OR (client->'scopes' @> '["openid","profile","email"]'::jsonb) IS DISTINCT FROM true OR (client->'redirectUris' ? ((l->>'appOrigin')||'/api/sso/callback')) IS DISTINCT FROM true THEN RETURN false; END IF;
   PERFORM id FROM webdock_auth.app_binding WHERE id=l->>'bindingID' AND client_id=l->>'clientID' AND enabled AND organization_id IS NULL FOR SHARE;
   IF NOT FOUND THEN RETURN false; END IF;
  END IF;
 ELSE RETURN false;
 END IF;
 RETURN true;
END $$;
ALTER FUNCTION webdock_rls.managed_mail_source(jsonb) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.managed_mail_source(jsonb) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.managed_mail_source(jsonb) TO webdock_worker_runtime;

-- This is an original, live action proof, never a generic job token. Auth is acquired by Rust first.
CREATE FUNCTION webdock_rls.managed_mail_action(invocation_id uuid, resource_input text) RETURNS jsonb
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE v webdock_auth.plugin_invocation%ROWTYPE; p webdock_auth.plugin_installation%ROWTYPE;
 g webdock_auth.plugin_grant%ROWTYPE; resource webdock_auth.plugin_resource%ROWTYPE; u webdock_auth."user"%ROWTYPE;
 a jsonb; auth jsonb; lineage jsonb; member_id text; proof jsonb; op boolean; consent jsonb; caller jsonb; issuer jsonb; caller_resource_grant_id text; issuer_resource_grant_id text;
BEGIN
 IF NOT webdock_rls.worker(ARRAY['mail-send-gateway']) THEN RETURN NULL; END IF;
 PERFORM pg_advisory_xact_lock_shared(hashtext('webdock-plugin-installation'));
 SELECT * INTO v FROM webdock_auth.plugin_invocation WHERE id=invocation_id AND plugin_id='mail' AND kind='action' AND name IN ('mail.send','mail.status','mail.cancel','mail.history','incoming.activate','mailboxes.create','domains.delete') AND revoked_at IS NULL AND expires_at>clock_timestamp() FOR SHARE;
 IF NOT FOUND OR (v.name<>'mailboxes.create' AND v.session_context->>'resourceID' IS DISTINCT FROM resource_input) OR resource_input IS NULL THEN RETURN NULL; END IF;
 SELECT * INTO p FROM webdock_auth.plugin_installation WHERE id='mail' AND kind='external' AND current_generation=v.runtime_generation FOR SHARE;
 IF NOT FOUND OR p.state NOT IN ('enabled','draining') OR (v.name='mail.send' AND p.state<>'enabled') THEN RETURN NULL; END IF;
 SELECT * INTO g FROM webdock_auth.plugin_grant WHERE plugin_id='mail' AND project_id=v.project_id AND customer_id=v.customer_id AND revision=v.grant_revision AND revoked_at IS NULL FOR SHARE;
 IF NOT FOUND THEN RETURN NULL; END IF;
 SELECT x INTO a FROM webdock_auth.plugin_release r,LATERAL jsonb_array_elements(r.manifest_bytes::jsonb->'actions') x WHERE r.plugin_id='mail' AND r.version=p.version AND x->>'name'=v.name;
 IF a IS NULL OR a->>'intent' IS DISTINCT FROM v.intent OR a->'capabilities' IS DISTINCT FROM v.capabilities OR (v.intent='write') IS DISTINCT FROM (v.name IN ('mail.send','mail.cancel','incoming.activate','mailboxes.create','domains.delete')) THEN RETURN NULL; END IF;
 SELECT * INTO resource FROM webdock_auth.plugin_resource WHERE plugin_id='mail' AND resource_id=resource_input AND offering_id='domain' AND project_id=v.project_id AND customer_id=v.customer_id FOR SHARE;
 IF NOT FOUND THEN RETURN NULL; END IF;
 SELECT * INTO u FROM webdock_auth."user" WHERE id=v.actor_id AND "emailVerified" AND NOT "mustChangePassword" AND NOT coalesce(banned,false) AND role IN ('user','operator') AND (role<>'operator' OR coalesce("twoFactorEnabled",false)) FOR SHARE;
 IF NOT FOUND THEN RETURN NULL; END IF;
 op:=u.role='operator' AND coalesce(u."twoFactorEnabled",false);
 IF NOT op THEN
  SELECT m.id INTO member_id FROM webdock_auth.member m JOIN webdock_auth.tenant_customer t ON t.organization_id=m."organizationId" WHERE m."userId"=v.actor_id AND t.customer_id=v.customer_id AND (m.role IN ('owner','admin') OR v.intent='read' AND m.role<>'') FOR SHARE OF m,t;
  IF NOT FOUND THEN RETURN NULL; END IF;
 END IF;
 auth:=v.session_context->'authority';
 IF auth->>'source'='browser' THEN
  lineage:=jsonb_build_object('source','browser');
  IF auth->'authority'->>'kind'='first-party-browser' THEN
   SELECT to_jsonb(c) INTO caller FROM webdock_auth."oauthClient" c WHERE c."clientId"=auth->'authority'->'browser'->>'client_id' FOR SHARE;
   IF caller IS NULL THEN RETURN NULL; END IF;
   lineage:=lineage||jsonb_build_object('clientID',caller->>'clientId','clientRevision',caller->>'updatedAt','appOrigin',auth->'authority'->'browser'->>'app_origin','bindingID',CASE WHEN jsonb_typeof(caller->'metadata')='string' THEN (caller->>'metadata')::jsonb->>'webdock_binding' ELSE caller->'metadata'->>'webdock_binding' END);
  END IF;
 ELSIF auth->>'source'='mcp' AND (auth->'authority'->>'write'='true' OR v.intent='read') THEN
  SELECT to_jsonb(c) INTO caller FROM webdock_auth."oauthClient" c WHERE c."clientId"=auth->'authority'->>'callerId' FOR SHARE;
  SELECT to_jsonb(c) INTO issuer FROM webdock_auth."oauthClient" c WHERE c."clientId"=auth->'authority'->>'issuerClientId' FOR SHARE;
  IF caller IS NULL OR issuer IS NULL THEN RETURN NULL; END IF;
  SELECT id INTO caller_resource_grant_id FROM webdock_auth."oauthClientResource" WHERE "clientId"=caller->>'clientId' AND "resourceId"=auth->'authority'->>'resource' FOR SHARE;
  IF NOT FOUND THEN RETURN NULL; END IF;
  SELECT id INTO issuer_resource_grant_id FROM webdock_auth."oauthClientResource" WHERE "clientId"=issuer->>'clientId' AND "resourceId"=auth->'authority'->>'resource' FOR SHARE;
  IF NOT FOUND THEN RETURN NULL; END IF;
  SELECT to_jsonb(c) INTO consent FROM webdock_auth."oauthConsent" c WHERE c."clientId"=auth->'authority'->>'issuerClientId' AND c."userId"=v.actor_id AND c.scopes ? CASE WHEN auth->'authority'->>'requiredScope' LIKE 'webdock:%' THEN 'webdock:write' ELSE 'hosting:write' END ORDER BY c.id LIMIT 1 FOR SHARE;
  IF consent IS NULL AND v.intent='write' AND issuer->>'skipConsent' IS DISTINCT FROM 'true' THEN RETURN NULL; END IF;
  lineage:=jsonb_build_object('source','mcp','callerID',caller->>'clientId','callerRevision',caller->>'updatedAt','issuerClientID',issuer->>'clientId','issuerRevision',issuer->>'updatedAt','callerResourceGrantID',caller_resource_grant_id,'issuerResourceGrantID',issuer_resource_grant_id,'resource',auth->'authority'->>'resource','writeScope',CASE WHEN auth->'authority'->>'requiredScope' LIKE 'webdock:%' THEN 'webdock:write' ELSE 'hosting:write' END,'consentID',consent->>'id','consentRevision',consent->>'updatedAt','skipConsent',coalesce((issuer->>'skipConsent')::boolean,false));
 ELSE RETURN NULL; END IF;
 proof:=jsonb_build_object('kind','action','action','mail.send','customerID',v.customer_id,'projectID',v.project_id,'resourceID',resource.resource_id,'domain',resource.name,'actorID',v.actor_id,'actorKind',CASE WHEN op THEN 'operator' ELSE 'member' END,'memberID',member_id,'grantRevision',g.revision,'policyRevision',p.policy_revision,'authorityLineage',lineage);
 IF v.name='mail.send' THEN
  IF NOT webdock_rls.managed_mail_source(proof) THEN RETURN NULL; END IF;
 ELSE
  PERFORM c.id FROM webdock_admin.customers c JOIN webdock_admin.projects j ON j.customer_id=c.id WHERE c.id=v.customer_id AND j.id=v.project_id AND c.status='active' AND j.status='active' AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=j.id AND d.status<>'deleted') FOR SHARE OF c,j;
  IF NOT FOUND OR NOT (a->>'audience'='customer' OR a->>'audience'='operator' AND op) OR NOT(g.capabilities @> (a->'capabilities')) THEN RETURN NULL; END IF;
  IF a->>'feature' IS NOT NULL AND (NOT webdock_rls.plugin_feature_supported('mail',a->>'feature') OR NOT op AND NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_feature_policy f WHERE f.plugin_id='mail' AND f.feature_id=a->>'feature' AND (f.mode='everyone' OR f.mode='selected' AND EXISTS(SELECT 1 FROM webdock_auth.plugin_feature_customer c WHERE c.plugin_id='mail' AND c.feature_id=f.feature_id AND c.customer_id=v.customer_id)))) THEN RETURN NULL; END IF;
 END IF;
 RETURN jsonb_build_object('invocation',to_jsonb(v),'source',proof,'action',a);
END $$;
ALTER FUNCTION webdock_rls.managed_mail_action(uuid,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.managed_mail_action(uuid,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.managed_mail_action(uuid,text) TO webdock_worker_runtime;

CREATE INDEX managed_mail_lane ON webdock_mail.send_job((source_context->>'executor'),not_before,id) WHERE state IN ('staging','queued','leased','handoff');
-- Immutable custody assignment; old workers are excluded in their Rust selectors.
CREATE FUNCTION webdock_mail.preserve_managed_custody() RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 IF OLD.source_context->>'executor'='managed-mail-v1' AND (NEW.id<>OLD.id OR NEW.customer_id<>OLD.customer_id OR NEW.payload_format<>OLD.payload_format OR NEW.idempotency_key<>OLD.idempotency_key OR NEW.source_context IS DISTINCT FROM OLD.source_context OR NEW.route IS DISTINCT FROM OLD.route OR NEW.envelope IS DISTINCT FROM OLD.envelope OR NEW.source_id<>OLD.source_id OR NEW.payload_ref<>OLD.payload_ref OR NEW.request_hash<>OLD.request_hash OR NEW.payload_bytes<>OLD.payload_bytes) THEN
  RAISE EXCEPTION 'Managed Mail custody is immutable' USING ERRCODE='23514';
 END IF; RETURN NEW;
END $$;
ALTER FUNCTION webdock_mail.preserve_managed_custody() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_mail.preserve_managed_custody() FROM PUBLIC;
CREATE TRIGGER managed_mail_custody BEFORE UPDATE ON webdock_mail.send_job FOR EACH ROW EXECUTE FUNCTION webdock_mail.preserve_managed_custody();
