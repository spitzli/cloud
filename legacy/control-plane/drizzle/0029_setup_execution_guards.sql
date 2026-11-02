
GRANT USAGE,CREATE ON SCHEMA webdock_rls TO webdock_platform_owner;
CREATE OR REPLACE FUNCTION webdock_rls.setup_allowed(customer text,project text DEFAULT NULL) RETURNS boolean LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
BEGIN
 IF NOT webdock_rls.worker(ARRAY['hosting-setup']) THEN RETURN false; END IF;
 RETURN EXISTS(
 SELECT 1 FROM webdock_auth.hosting_setup s
 JOIN webdock_admin.projects p ON p.id=s.project_id
 JOIN webdock_admin.customers c ON c.id=s.customer_id
 JOIN webdock_auth.hosting_project h ON h.project_id=p.id
 JOIN webdock_auth.tenant_customer t ON t.customer_id=c.id
 JOIN webdock_auth."user" u ON u.id=s.actor->>'subject'
 JOIN webdock_auth.session session ON session.id=s.actor->>'sessionID' AND session."userId"=u.id
 LEFT JOIN webdock_auth.member m ON m."userId"=u.id AND m."organizationId"=t.organization_id
 WHERE s.id=webdock_rls.request_scope()->>'setupID' AND s.generation::text=webdock_rls.request_scope()->>'generation'
 AND s.customer_id=customer AND (project IS NULL OR s.project_id=project) AND s.customer_id=webdock_rls.request_scope()->>'customerID'
 AND s.state='running' AND s.confirmed_preview_id IS NOT NULL AND s.lease_until>now()
 AND s.actor->'scopes' ? 'hosting:write' AND s.actor->'scopes' ? 'hosting:read'
 AND c.status='active' AND p.status='active' AND session."expiresAt">now()
 AND NOT coalesce(u.banned,false) AND u."emailVerified" AND NOT u."mustChangePassword"
 AND ((u.role='operator' AND u."twoFactorEnabled") OR (u.role='user' AND m.role IN ('owner','admin') AND h.mode='selfservice'))
 AND NOT EXISTS(SELECT 1 FROM webdock_auth.studio_tenant_preview preview WHERE preview.session_id=session.id)
 AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion deletion WHERE deletion.project_id=s.project_id AND deletion.status<>'deleted'));
END $$;
CREATE OR REPLACE FUNCTION webdock_rls.setup_identity(subject text,session text DEFAULT NULL) RETURNS boolean LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
BEGIN
 IF NOT webdock_rls.worker(ARRAY['hosting-setup']) THEN RETURN false; END IF;
 RETURN EXISTS(SELECT 1 FROM webdock_auth.hosting_setup s WHERE s.id=webdock_rls.request_scope()->>'setupID' AND s.actor->>'subject'=setup_identity.subject AND (setup_identity.session IS NULL OR s.actor->>'sessionID'=setup_identity.session) AND webdock_rls.setup_allowed(s.customer_id::text,s.project_id::text));
END $$;
CREATE OR REPLACE FUNCTION webdock_rls.setup_cluster(cluster text) RETURNS boolean LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
BEGIN
 IF NOT webdock_rls.worker(ARRAY['hosting-setup']) THEN RETURN false; END IF;
 RETURN EXISTS(SELECT 1 FROM webdock_auth.hosting_setup s JOIN webdock_auth.hosting_project h ON h.project_id=s.project_id WHERE s.id=webdock_rls.request_scope()->>'setupID' AND h.cluster_id=cluster AND webdock_rls.setup_allowed(s.customer_id::text,s.project_id::text));
END $$;
ALTER FUNCTION webdock_rls.setup_allowed(text,text) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.setup_identity(text,text) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.setup_cluster(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.setup_allowed(text,text),webdock_rls.setup_identity(text,text),webdock_rls.setup_cluster(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.setup_allowed(text,text),webdock_rls.setup_identity(text,text),webdock_rls.setup_cluster(text) TO webdock_worker_runtime,webdock_auth_runtime,webdock_policy_owner;
REVOKE CREATE ON SCHEMA webdock_rls FROM webdock_platform_owner;

DROP POLICY setup_execution ON webdock_auth.hosting_project;
CREATE POLICY setup_execution_read ON webdock_auth.hosting_project FOR SELECT TO webdock_worker_runtime USING(webdock_rls.setup_allowed(customer_id::text,project_id::text));
DROP POLICY setup_execution ON webdock_auth.hosting_database;
CREATE POLICY setup_execution_read ON webdock_auth.hosting_database FOR SELECT TO webdock_worker_runtime USING(webdock_rls.setup_allowed(customer_id::text,project_id::text));
DROP POLICY setup_execution ON webdock_auth.hosting_database_offer;
CREATE POLICY setup_execution_read ON webdock_auth.hosting_database_offer FOR SELECT TO webdock_worker_runtime USING(webdock_rls.setup_allowed(customer_id::text,project_id::text));
DROP POLICY setup_execution ON webdock_auth.hosting_database_order;
CREATE POLICY setup_execution_read ON webdock_auth.hosting_database_order FOR SELECT TO webdock_worker_runtime USING(webdock_rls.setup_allowed(customer_id::text,project_id::text));
DO $migration$ DECLARE definition text; BEGIN
 SELECT pg_get_functiondef(t.tgfoid) INTO definition FROM pg_trigger t WHERE t.tgrelid='webdock_auth.hosting_setup'::regclass AND t.tgname='aa_webdock_rls_old_write';
 IF definition IS NOT NULL THEN
  IF position('BEGIN'||chr(10) IN definition)=0 THEN RAISE EXCEPTION 'Unrecognized original-row policy guard'; END IF;
  EXECUTE replace(definition,'BEGIN'||chr(10),'BEGIN'||chr(10)||' IF session_user=''webdock_worker_runtime'' AND coalesce((webdock_rls.worker(ARRAY[''hosting-setup-dispatch''])),false) THEN RETURN NEW; END IF;
');
 END IF;
END $migration$;
DO $migration$ DECLARE definition text; BEGIN
 SELECT pg_get_functiondef(t.tgfoid) INTO definition FROM pg_trigger t WHERE t.tgrelid='webdock_auth.hosting_app'::regclass AND t.tgname='aa_webdock_rls_old_write';
 IF definition IS NOT NULL THEN
  IF position('BEGIN'||chr(10) IN definition)=0 THEN RAISE EXCEPTION 'Unrecognized original-row policy guard'; END IF;
  EXECUTE replace(definition,'BEGIN'||chr(10),'BEGIN'||chr(10)||' IF session_user=''webdock_worker_runtime'' AND coalesce((EXISTS(SELECT 1 FROM webdock_auth.hosting_project p WHERE p.project_id::text=OLD."project_id"::text AND webdock_rls.setup_allowed(p.customer_id::text,p.project_id::text))),false) THEN RETURN NEW; END IF;
');
 END IF;
END $migration$;
DO $migration$ DECLARE definition text; BEGIN
 SELECT pg_get_functiondef(t.tgfoid) INTO definition FROM pg_trigger t WHERE t.tgrelid='webdock_auth.hosting_service_binding'::regclass AND t.tgname='aa_webdock_rls_old_write';
 IF definition IS NOT NULL THEN
  IF position('BEGIN'||chr(10) IN definition)=0 THEN RAISE EXCEPTION 'Unrecognized original-row policy guard'; END IF;
  EXECUTE replace(definition,'BEGIN'||chr(10),'BEGIN'||chr(10)||' IF session_user=''webdock_worker_runtime'' AND coalesce((webdock_rls.setup_allowed(OLD."customer_id"::text,OLD."project_id"::text)),false) THEN RETURN NEW; END IF;
');
 END IF;
END $migration$;
DO $migration$ DECLARE definition text; BEGIN
 SELECT pg_get_functiondef(t.tgfoid) INTO definition FROM pg_trigger t WHERE t.tgrelid='webdock_auth.hosting_operation'::regclass AND t.tgname='aa_webdock_rls_old_write';
 IF definition IS NOT NULL THEN
  IF position('BEGIN'||chr(10) IN definition)=0 THEN RAISE EXCEPTION 'Unrecognized original-row policy guard'; END IF;
  EXECUTE replace(definition,'BEGIN'||chr(10),'BEGIN'||chr(10)||' IF session_user=''webdock_worker_runtime'' AND coalesce((webdock_rls.setup_allowed(OLD."customer_id"::text,OLD."project_id"::text)),false) THEN RETURN NEW; END IF;
');
 END IF;
END $migration$;
DO $migration$ DECLARE definition text; BEGIN
 SELECT pg_get_functiondef(t.tgfoid) INTO definition FROM pg_trigger t WHERE t.tgrelid='webdock_auth.hosting_reservation'::regclass AND t.tgname='aa_webdock_rls_old_write';
 IF definition IS NOT NULL THEN
  IF position('BEGIN'||chr(10) IN definition)=0 THEN RAISE EXCEPTION 'Unrecognized original-row policy guard'; END IF;
  EXECUTE replace(definition,'BEGIN'||chr(10),'BEGIN'||chr(10)||' IF session_user=''webdock_worker_runtime'' AND coalesce((webdock_rls.setup_allowed(OLD."customer_id"::text,OLD."project_id"::text)),false) THEN RETURN NEW; END IF;
');
 END IF;
END $migration$;
DO $migration$ DECLARE definition text; BEGIN
 SELECT pg_get_functiondef(t.tgfoid) INTO definition FROM pg_trigger t WHERE t.tgrelid='webdock_auth.git_source'::regclass AND t.tgname='aa_webdock_rls_old_write';
 IF definition IS NOT NULL THEN
  IF position('BEGIN'||chr(10) IN definition)=0 THEN RAISE EXCEPTION 'Unrecognized original-row policy guard'; END IF;
  EXECUTE replace(definition,'BEGIN'||chr(10),'BEGIN'||chr(10)||' IF session_user=''webdock_worker_runtime'' AND coalesce((webdock_rls.setup_allowed(OLD."customer_id"::text,OLD."project_id"::text)),false) THEN RETURN NEW; END IF;
');
 END IF;
END $migration$;
DO $migration$ DECLARE definition text; BEGIN
 SELECT pg_get_functiondef(t.tgfoid) INTO definition FROM pg_trigger t WHERE t.tgrelid='webdock_auth.git_build'::regclass AND t.tgname='aa_webdock_rls_old_write';
 IF definition IS NOT NULL THEN
  IF position('BEGIN'||chr(10) IN definition)=0 THEN RAISE EXCEPTION 'Unrecognized original-row policy guard'; END IF;
  EXECUTE replace(definition,'BEGIN'||chr(10),'BEGIN'||chr(10)||' IF session_user=''webdock_worker_runtime'' AND coalesce((webdock_rls.setup_allowed(OLD."customer_id"::text,OLD."project_id"::text)),false) THEN RETURN NEW; END IF;
');
 END IF;
END $migration$;
DO $migration$ DECLARE definition text; BEGIN
 SELECT pg_get_functiondef(t.tgfoid) INTO definition FROM pg_trigger t WHERE t.tgrelid='webdock_auth.git_artifact'::regclass AND t.tgname='aa_webdock_rls_old_write';
 IF definition IS NOT NULL THEN
  IF position('BEGIN'||chr(10) IN definition)=0 THEN RAISE EXCEPTION 'Unrecognized original-row policy guard'; END IF;
  EXECUTE replace(definition,'BEGIN'||chr(10),'BEGIN'||chr(10)||' IF session_user=''webdock_worker_runtime'' AND coalesce((webdock_rls.setup_allowed(OLD."customer_id"::text,OLD."project_id"::text)),false) THEN RETURN NEW; END IF;
');
 END IF;
END $migration$;
DO $migration$ DECLARE definition text; BEGIN
 SELECT pg_get_functiondef(t.tgfoid) INTO definition FROM pg_trigger t WHERE t.tgrelid='webdock_auth.git_release'::regclass AND t.tgname='aa_webdock_rls_old_write';
 IF definition IS NOT NULL THEN
  IF position('BEGIN'||chr(10) IN definition)=0 THEN RAISE EXCEPTION 'Unrecognized original-row policy guard'; END IF;
  EXECUTE replace(definition,'BEGIN'||chr(10),'BEGIN'||chr(10)||' IF session_user=''webdock_worker_runtime'' AND coalesce((webdock_rls.setup_allowed(OLD."customer_id"::text,OLD."project_id"::text)),false) THEN RETURN NEW; END IF;
');
 END IF;
END $migration$;
DO $migration$ DECLARE definition text; BEGIN
 SELECT pg_get_functiondef(t.tgfoid) INTO definition FROM pg_trigger t WHERE t.tgrelid='webdock_auth.git_check_outbox'::regclass AND t.tgname='aa_webdock_rls_old_write';
 IF definition IS NOT NULL THEN
  IF position('BEGIN'||chr(10) IN definition)=0 THEN RAISE EXCEPTION 'Unrecognized original-row policy guard'; END IF;
  EXECUTE replace(definition,'BEGIN'||chr(10),'BEGIN'||chr(10)||' IF session_user=''webdock_worker_runtime'' AND coalesce((EXISTS(SELECT 1 FROM webdock_auth.git_build b WHERE b.id::text=OLD."build_id"::text AND webdock_rls.setup_allowed(b.customer_id::text,b.project_id::text))),false) THEN RETURN NEW; END IF;
');
 END IF;
END $migration$;
DO $migration$ DECLARE definition text; BEGIN
 SELECT pg_get_functiondef(t.tgfoid) INTO definition FROM pg_trigger t WHERE t.tgrelid='webdock_auth.mail_api_key'::regclass AND t.tgname='aa_webdock_rls_old_write';
 IF definition IS NOT NULL THEN
  IF position('BEGIN'||chr(10) IN definition)=0 THEN RAISE EXCEPTION 'Unrecognized original-row policy guard'; END IF;
  EXECUTE replace(definition,'BEGIN'||chr(10),'BEGIN'||chr(10)||' IF session_user=''webdock_worker_runtime'' AND coalesce((EXISTS(SELECT 1 FROM webdock_auth.hosting_service_binding b WHERE (b.mail_key_id=OLD."id"::text OR b.retiring_mail_key_id=OLD."id"::text) AND webdock_rls.setup_allowed(b.customer_id::text,b.project_id::text))),false) THEN RETURN NEW; END IF;
');
 END IF;
END $migration$;
