-- Separate PL/pgSQL branches avoid planning worker-only subqueries for authentication roles.
DO $migration$ DECLARE definition text; BEGIN
 SELECT pg_get_functiondef(t.tgfoid) INTO definition FROM pg_trigger t WHERE t.tgrelid='webdock_auth.hosting_setup'::regclass AND t.tgname='aa_webdock_rls_old_write';
 IF definition IS NOT NULL THEN EXECUTE replace(definition,' IF session_user=''webdock_worker_runtime'' AND coalesce((webdock_rls.worker(ARRAY[''hosting-setup-dispatch''])),false) THEN RETURN NEW; END IF;
',' IF session_user=''webdock_worker_runtime'' THEN
  IF coalesce((webdock_rls.worker(ARRAY[''hosting-setup-dispatch''])),false) THEN RETURN NEW; END IF;
 END IF;
'); END IF;
END $migration$;
DO $migration$ DECLARE definition text; BEGIN
 SELECT pg_get_functiondef(t.tgfoid) INTO definition FROM pg_trigger t WHERE t.tgrelid='webdock_auth.hosting_app'::regclass AND t.tgname='aa_webdock_rls_old_write';
 IF definition IS NOT NULL THEN EXECUTE replace(definition,' IF session_user=''webdock_worker_runtime'' AND coalesce((EXISTS(SELECT 1 FROM webdock_auth.hosting_project p WHERE p.project_id::text=OLD."project_id"::text AND webdock_rls.setup_allowed(p.customer_id::text,p.project_id::text))),false) THEN RETURN NEW; END IF;
',' IF session_user=''webdock_worker_runtime'' THEN
  IF coalesce((EXISTS(SELECT 1 FROM webdock_auth.hosting_project p WHERE p.project_id::text=OLD."project_id"::text AND webdock_rls.setup_allowed(p.customer_id::text,p.project_id::text))),false) THEN RETURN NEW; END IF;
 END IF;
'); END IF;
END $migration$;
DO $migration$ DECLARE definition text; BEGIN
 SELECT pg_get_functiondef(t.tgfoid) INTO definition FROM pg_trigger t WHERE t.tgrelid='webdock_auth.hosting_service_binding'::regclass AND t.tgname='aa_webdock_rls_old_write';
 IF definition IS NOT NULL THEN EXECUTE replace(definition,' IF session_user=''webdock_worker_runtime'' AND coalesce((webdock_rls.setup_allowed(OLD."customer_id"::text,OLD."project_id"::text)),false) THEN RETURN NEW; END IF;
',' IF session_user=''webdock_worker_runtime'' THEN
  IF coalesce((webdock_rls.setup_allowed(OLD."customer_id"::text,OLD."project_id"::text)),false) THEN RETURN NEW; END IF;
 END IF;
'); END IF;
END $migration$;
DO $migration$ DECLARE definition text; BEGIN
 SELECT pg_get_functiondef(t.tgfoid) INTO definition FROM pg_trigger t WHERE t.tgrelid='webdock_auth.hosting_operation'::regclass AND t.tgname='aa_webdock_rls_old_write';
 IF definition IS NOT NULL THEN EXECUTE replace(definition,' IF session_user=''webdock_worker_runtime'' AND coalesce((webdock_rls.setup_allowed(OLD."customer_id"::text,OLD."project_id"::text)),false) THEN RETURN NEW; END IF;
',' IF session_user=''webdock_worker_runtime'' THEN
  IF coalesce((webdock_rls.setup_allowed(OLD."customer_id"::text,OLD."project_id"::text)),false) THEN RETURN NEW; END IF;
 END IF;
'); END IF;
END $migration$;
DO $migration$ DECLARE definition text; BEGIN
 SELECT pg_get_functiondef(t.tgfoid) INTO definition FROM pg_trigger t WHERE t.tgrelid='webdock_auth.hosting_reservation'::regclass AND t.tgname='aa_webdock_rls_old_write';
 IF definition IS NOT NULL THEN EXECUTE replace(definition,' IF session_user=''webdock_worker_runtime'' AND coalesce((webdock_rls.setup_allowed(OLD."customer_id"::text,OLD."project_id"::text)),false) THEN RETURN NEW; END IF;
',' IF session_user=''webdock_worker_runtime'' THEN
  IF coalesce((webdock_rls.setup_allowed(OLD."customer_id"::text,OLD."project_id"::text)),false) THEN RETURN NEW; END IF;
 END IF;
'); END IF;
END $migration$;
DO $migration$ DECLARE definition text; BEGIN
 SELECT pg_get_functiondef(t.tgfoid) INTO definition FROM pg_trigger t WHERE t.tgrelid='webdock_auth.git_source'::regclass AND t.tgname='aa_webdock_rls_old_write';
 IF definition IS NOT NULL THEN EXECUTE replace(definition,' IF session_user=''webdock_worker_runtime'' AND coalesce((webdock_rls.setup_allowed(OLD."customer_id"::text,OLD."project_id"::text)),false) THEN RETURN NEW; END IF;
',' IF session_user=''webdock_worker_runtime'' THEN
  IF coalesce((webdock_rls.setup_allowed(OLD."customer_id"::text,OLD."project_id"::text)),false) THEN RETURN NEW; END IF;
 END IF;
'); END IF;
END $migration$;
DO $migration$ DECLARE definition text; BEGIN
 SELECT pg_get_functiondef(t.tgfoid) INTO definition FROM pg_trigger t WHERE t.tgrelid='webdock_auth.git_build'::regclass AND t.tgname='aa_webdock_rls_old_write';
 IF definition IS NOT NULL THEN EXECUTE replace(definition,' IF session_user=''webdock_worker_runtime'' AND coalesce((webdock_rls.setup_allowed(OLD."customer_id"::text,OLD."project_id"::text)),false) THEN RETURN NEW; END IF;
',' IF session_user=''webdock_worker_runtime'' THEN
  IF coalesce((webdock_rls.setup_allowed(OLD."customer_id"::text,OLD."project_id"::text)),false) THEN RETURN NEW; END IF;
 END IF;
'); END IF;
END $migration$;
DO $migration$ DECLARE definition text; BEGIN
 SELECT pg_get_functiondef(t.tgfoid) INTO definition FROM pg_trigger t WHERE t.tgrelid='webdock_auth.git_artifact'::regclass AND t.tgname='aa_webdock_rls_old_write';
 IF definition IS NOT NULL THEN EXECUTE replace(definition,' IF session_user=''webdock_worker_runtime'' AND coalesce((webdock_rls.setup_allowed(OLD."customer_id"::text,OLD."project_id"::text)),false) THEN RETURN NEW; END IF;
',' IF session_user=''webdock_worker_runtime'' THEN
  IF coalesce((webdock_rls.setup_allowed(OLD."customer_id"::text,OLD."project_id"::text)),false) THEN RETURN NEW; END IF;
 END IF;
'); END IF;
END $migration$;
DO $migration$ DECLARE definition text; BEGIN
 SELECT pg_get_functiondef(t.tgfoid) INTO definition FROM pg_trigger t WHERE t.tgrelid='webdock_auth.git_release'::regclass AND t.tgname='aa_webdock_rls_old_write';
 IF definition IS NOT NULL THEN EXECUTE replace(definition,' IF session_user=''webdock_worker_runtime'' AND coalesce((webdock_rls.setup_allowed(OLD."customer_id"::text,OLD."project_id"::text)),false) THEN RETURN NEW; END IF;
',' IF session_user=''webdock_worker_runtime'' THEN
  IF coalesce((webdock_rls.setup_allowed(OLD."customer_id"::text,OLD."project_id"::text)),false) THEN RETURN NEW; END IF;
 END IF;
'); END IF;
END $migration$;
DO $migration$ DECLARE definition text; BEGIN
 SELECT pg_get_functiondef(t.tgfoid) INTO definition FROM pg_trigger t WHERE t.tgrelid='webdock_auth.git_check_outbox'::regclass AND t.tgname='aa_webdock_rls_old_write';
 IF definition IS NOT NULL THEN EXECUTE replace(definition,' IF session_user=''webdock_worker_runtime'' AND coalesce((EXISTS(SELECT 1 FROM webdock_auth.git_build b WHERE b.id::text=OLD."build_id"::text AND webdock_rls.setup_allowed(b.customer_id::text,b.project_id::text))),false) THEN RETURN NEW; END IF;
',' IF session_user=''webdock_worker_runtime'' THEN
  IF coalesce((EXISTS(SELECT 1 FROM webdock_auth.git_build b WHERE b.id::text=OLD."build_id"::text AND webdock_rls.setup_allowed(b.customer_id::text,b.project_id::text))),false) THEN RETURN NEW; END IF;
 END IF;
'); END IF;
END $migration$;
DO $migration$ DECLARE definition text; BEGIN
 SELECT pg_get_functiondef(t.tgfoid) INTO definition FROM pg_trigger t WHERE t.tgrelid='webdock_auth.mail_api_key'::regclass AND t.tgname='aa_webdock_rls_old_write';
 IF definition IS NOT NULL THEN EXECUTE replace(definition,' IF session_user=''webdock_worker_runtime'' AND coalesce((EXISTS(SELECT 1 FROM webdock_auth.hosting_service_binding b WHERE (b.mail_key_id=OLD."id"::text OR b.retiring_mail_key_id=OLD."id"::text) AND webdock_rls.setup_allowed(b.customer_id::text,b.project_id::text))),false) THEN RETURN NEW; END IF;
',' IF session_user=''webdock_worker_runtime'' THEN
  IF coalesce((EXISTS(SELECT 1 FROM webdock_auth.hosting_service_binding b WHERE (b.mail_key_id=OLD."id"::text OR b.retiring_mail_key_id=OLD."id"::text) AND webdock_rls.setup_allowed(b.customer_id::text,b.project_id::text))),false) THEN RETURN NEW; END IF;
 END IF;
'); END IF;
END $migration$;
