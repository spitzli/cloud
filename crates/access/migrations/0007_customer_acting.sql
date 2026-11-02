-- Native0038: explicit, exact-session customer-owner acting. Existing previews stay read-only.
ALTER TABLE webdock_auth.studio_tenant_preview ADD COLUMN mode text NOT NULL DEFAULT 'read_only'
 CHECK (mode IN ('read_only','owner'));
ALTER TABLE webdock_auth.studio_tenant_preview ADD COLUMN scope_id uuid NOT NULL DEFAULT gen_random_uuid();

-- This deliberately retains actor()/operator() suppression and never creates a membership.
CREATE FUNCTION webdock_rls.acting_customer() RETURNS text
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
 SELECT p.customer_id FROM webdock_auth.studio_tenant_preview p
 JOIN webdock_admin.customers c ON c.id=p.customer_id AND c.status='active'
 JOIN webdock_auth.tenant_customer t ON t.customer_id=p.customer_id AND t.organization_id=p.organization_id
 WHERE p.actor_id=webdock_rls.actor()->>'subject' AND p.session_id=webdock_rls.actor()->>'sessionID'
 AND webdock_rls.request_scope()->>'authProvider'='better-auth'
 AND coalesce((webdock_rls.actor()->>'root')::boolean,false)
 AND p.mode='owner' AND p.expired_at IS NULL AND p.expires_at>statement_timestamp()
 AND webdock_rls.native_organization_authority(p.organization_id)
$$;
ALTER FUNCTION webdock_rls.acting_customer() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.acting_customer() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.acting_customer() TO webdock_auth_runtime,webdock_registry_runtime,webdock_policy_owner,webdock_worker_runtime;

-- A fresh random epoch prevents an exited scope from reviving captured work on re-entry.
CREATE FUNCTION webdock_rls.customer_scope() RETURNS text
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
 SELECT md5(concat_ws(':',p.scope_id,p.customer_id,p.organization_id,p.mode))
 FROM webdock_auth.studio_tenant_preview p
 WHERE p.actor_id=webdock_rls.actor()->>'subject' AND p.session_id=webdock_rls.actor()->>'sessionID'
$$;
ALTER FUNCTION webdock_rls.customer_scope() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.customer_scope() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.customer_scope() TO webdock_auth_runtime,webdock_registry_runtime,webdock_policy_owner,webdock_worker_runtime;

CREATE FUNCTION webdock_auth.guard_customer_scope() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog AS $$ BEGIN
 IF session_user IN ('webdock_auth_runtime','webdock_registry_runtime') AND
 (NEW.scope_id IS DISTINCT FROM OLD.scope_id OR NEW.mode IS DISTINCT FROM OLD.mode
 OR NEW.customer_id IS DISTINCT FROM OLD.customer_id OR NEW.organization_id IS DISTINCT FROM OLD.organization_id
 OR NEW.session_id IS DISTINCT FROM OLD.session_id OR NEW.actor_id IS DISTINCT FROM OLD.actor_id
 OR NEW.expires_at IS DISTINCT FROM OLD.expires_at OR NEW.created_at IS DISTINCT FROM OLD.created_at
 OR OLD.expired_at IS NOT NULL AND NEW.expired_at IS DISTINCT FROM OLD.expired_at)
 THEN RAISE EXCEPTION 'Customer scope is immutable; exit and start a new scope' USING ERRCODE='42501'; END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_auth.guard_customer_scope() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.guard_customer_scope() FROM PUBLIC;
CREATE TRIGGER guard_customer_scope BEFORE UPDATE ON webdock_auth.studio_tenant_preview
 FOR EACH ROW EXECUTE FUNCTION webdock_auth.guard_customer_scope();

-- Preserve reviewed external-proof and worker branches. Unknown policy text fails the migration.
DO $acting$ DECLARE definition text; old text; BEGIN
 SELECT pg_get_functiondef('webdock_rls.customer_allowed(text,boolean)'::regprocedure) INTO definition;
 old:='IF a->''preview''<>''null''::jsonb THEN RETURN NOT writing AND';
 IF strpos(definition,old)=0 THEN RAISE EXCEPTION 'Unrecognized customer preview policy'; END IF;
 EXECUTE replace(definition,old,'IF a->''preview''<>''null''::jsonb THEN RETURN (NOT writing OR webdock_rls.acting_customer()=customer) AND coalesce((a->>''root'')::boolean,false) AND webdock_rls.native_organization_authority(a->''preview''->>''organization_id'') AND');
 SELECT pg_get_functiondef('webdock_rls.membership_role(text)'::regprocedure) INTO definition;
 old:='IF a ? ''clerk'' THEN';
 IF strpos(definition,old)=0 THEN RAISE EXCEPTION 'Unrecognized membership policy'; END IF;
 EXECUTE replace(definition,old,'IF a->''preview''<>''null''::jsonb THEN RETURN CASE WHEN EXISTS(SELECT 1 FROM webdock_auth.tenant_customer t WHERE t.customer_id=webdock_rls.acting_customer() AND t.organization_id=org) THEN ''owner'' ELSE NULL END; END IF; '||old);
 SELECT pg_get_functiondef('webdock_rls.customer_permission(text,text,boolean)'::regprocedure) INTO definition;
 old:='IF a IS NULL OR a->''preview''<>''null''::jsonb THEN RETURN false; END IF;';
 IF strpos(definition,old)=0 THEN RAISE EXCEPTION 'Unrecognized customer permission policy'; END IF;
 EXECUTE replace(definition,old,'IF a IS NULL THEN RETURN false; END IF; IF a->''preview''<>''null''::jsonb THEN RETURN coalesce(webdock_rls.acting_customer()=customer,false); END IF;');
 SELECT pg_get_functiondef('webdock_rls.plugin_context(text)'::regprocedure) INTO definition;
 old:='OR actor->''preview'' IS DISTINCT FROM ''null''::jsonb';
 IF strpos(definition,old)=0 THEN RAISE EXCEPTION 'Unrecognized plugin preview policy'; END IF;
 EXECUTE replace(definition,old,'OR invocation.session_context->>''customerScope'' IS DISTINCT FROM webdock_rls.customer_scope() OR (actor->''preview'' IS DISTINCT FROM ''null''::jsonb AND webdock_rls.acting_customer() IS DISTINCT FROM invocation.customer_id)');
END $acting$;

CREATE OR REPLACE FUNCTION webdock_rls.create_customer_project(project text,project_name text,customer text) RETURNS boolean
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$ BEGIN
 IF webdock_rls.actor() IS NULL OR NOT webdock_rls.customer_allowed(customer,true)
 OR NOT EXISTS(SELECT 1 FROM webdock_admin.customers WHERE id=customer AND status='active') THEN RETURN false; END IF;
 INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES(project,project_name,customer);
 RETURN true;
END $$;

CREATE OR REPLACE FUNCTION webdock_rls.access_preview() RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
 SELECT jsonb_build_object('customerID',p.customer_id,'customerName',c.name,'organizationID',p.organization_id,'expiresAt',p.expires_at,
 'status',CASE WHEN p.expired_at IS NULL AND p.expires_at>statement_timestamp() AND c.status='active' AND t.organization_id=p.organization_id AND webdock_rls.native_organization_authority(p.organization_id) THEN 'active' ELSE 'expired' END,
 'mode',p.mode,'readOnly',webdock_rls.acting_customer() IS DISTINCT FROM p.customer_id,'role',CASE WHEN webdock_rls.acting_customer()=p.customer_id THEN 'owner' ELSE 'admin' END)
 FROM webdock_auth.studio_tenant_preview p JOIN webdock_admin.customers c ON c.id=p.customer_id LEFT JOIN webdock_auth.tenant_customer t ON t.customer_id=p.customer_id
 WHERE p.session_id=webdock_rls.actor()->>'sessionID' AND p.actor_id=webdock_rls.subject() AND coalesce((webdock_rls.actor()->>'root')::boolean,false)
$$;

-- Tokens are independent sessions and must never inherit scoped operator authority.
ALTER POLICY customer_manage ON webdock_auth.api_token
 USING (webdock_rls.actor()->'preview'='null'::jsonb AND webdock_rls.customer_allowed(customer_id,true))
 WITH CHECK (webdock_rls.actor()->'preview'='null'::jsonb AND webdock_rls.customer_allowed(customer_id,true));

-- Add the scoped local proof kind without weakening any other launch row constraints.
DO $proof_shapes$ DECLARE c record; definition text; BEGIN
 FOR c IN SELECT n.nspname,t.relname,pc.conname,pg_get_constraintdef(pc.oid) AS definition
 FROM pg_constraint pc JOIN pg_class t ON t.oid=pc.conrelid JOIN pg_namespace n ON n.oid=t.relnamespace
 WHERE n.nspname IN ('webdock_auth','webdock_mail') AND pc.contype='c'
 AND t.relname IN ('database_launch','database_session','managed_database_launch','managed_database_session','webmail_launch','webmail_session','native_operation','native_configuration_operation') LOOP
  IF strpos(c.definition,'local-session')>0 AND strpos(c.definition,'first-party-browser')>0 THEN
   definition:=replace(c.definition,'''first-party-browser''::text','''first-party-browser''::text, ''customer-session''::text');
   IF c.relname='native_configuration_operation' THEN
    definition:=replace(definition,'(admission_member_id IS NOT NULL)','((admission_member_id IS NOT NULL) OR (native_authority->>''kind''=''customer-session'') OR (native_authority#>>''{browser,customer_scope}'' IS NOT NULL))');
   END IF;
   IF definition=c.definition THEN RAISE EXCEPTION 'Unrecognized native authority constraint'; END IF;
   EXECUTE format('ALTER TABLE %I.%I DROP CONSTRAINT %I',c.nspname,c.relname,c.conname);
   EXECUTE format('ALTER TABLE %I.%I ADD CONSTRAINT %I %s',c.nspname,c.relname,c.conname,definition);
  END IF;
 END LOOP;
END $proof_shapes$;

CREATE FUNCTION webdock_rls.first_party_customer_authority(proof jsonb) RETURNS boolean
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
 SELECT webdock_rls.actor()->'preview'='null'::jsonb OR
 (webdock_rls.acting_customer() IS NOT NULL AND webdock_rls.customer_scope()=
 coalesce(proof->>'customer_scope',proof#>>'{browser,customer_scope}'))
$$;
ALTER FUNCTION webdock_rls.first_party_customer_authority(jsonb) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.first_party_customer_authority(jsonb) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.first_party_customer_authority(jsonb) TO webdock_auth_runtime,webdock_policy_owner;

-- Existing target/grant/session predicates remain intact for both database launch paths.
DO $launch_policies$ DECLARE p record; expression text; condition text; definition text; BEGIN
 FOR p IN SELECT tablename,policyname,qual,with_check FROM pg_policies
 WHERE schemaname='webdock_auth' AND tablename IN ('database_launch','managed_database_launch') AND policyname LIKE 'webdock_auth_runtime_%' LOOP
  expression:='';
  IF p.qual IS NOT NULL THEN
   condition:=regexp_replace(p.qual,'\(\(webdock_rls.actor\(\) -> ''preview''::text\) = ''null''::jsonb\)','webdock_rls.first_party_customer_authority(native_authority)','g');
   expression:=expression||' USING ('||condition||')';
  END IF;
  IF p.with_check IS NOT NULL THEN
   condition:=regexp_replace(p.with_check,'\(\(webdock_rls.actor\(\) -> ''preview''::text\) = ''null''::jsonb\)','webdock_rls.first_party_customer_authority(native_authority)','g');
   expression:=expression||' WITH CHECK ('||condition||')';
  END IF;
  EXECUTE format('ALTER POLICY %I ON webdock_auth.%I%s',p.policyname,p.tablename,expression);
 END LOOP;
 FOR p IN SELECT DISTINCT t.tgfoid FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid JOIN pg_namespace n ON n.oid=c.relnamespace
 WHERE n.nspname='webdock_auth' AND c.relname IN ('database_launch','managed_database_launch') AND t.tgname='aa_webdock_rls_old_write' LOOP
  definition:=pg_get_functiondef(p.tgfoid);
  EXECUTE replace(definition,'webdock_rls.actor()->''preview''=''null''::jsonb','webdock_rls.first_party_customer_authority(OLD.native_authority)');
 END LOOP;
END $launch_policies$;
