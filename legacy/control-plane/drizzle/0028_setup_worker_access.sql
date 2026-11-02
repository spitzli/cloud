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
 RETURN EXISTS(SELECT 1 FROM webdock_auth.hosting_setup s WHERE s.id=webdock_rls.request_scope()->>'setupID' AND s.actor->>'subject'=subject AND (session IS NULL OR s.actor->>'sessionID'=session) AND webdock_rls.setup_allowed(s.customer_id::text,s.project_id::text));
END $$;
CREATE OR REPLACE FUNCTION webdock_rls.setup_cluster(cluster text) RETURNS boolean LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
BEGIN
 RETURN EXISTS(SELECT 1 FROM webdock_auth.hosting_setup s JOIN webdock_auth.hosting_project h ON h.project_id=s.project_id WHERE s.id=webdock_rls.request_scope()->>'setupID' AND h.cluster_id=cluster AND webdock_rls.setup_allowed(s.customer_id::text,s.project_id::text));
END $$;
ALTER FUNCTION webdock_rls.setup_allowed(text,text) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.setup_identity(text,text) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.setup_cluster(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.setup_allowed(text,text),webdock_rls.setup_identity(text,text),webdock_rls.setup_cluster(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.setup_allowed(text,text),webdock_rls.setup_identity(text,text),webdock_rls.setup_cluster(text) TO webdock_worker_runtime,webdock_auth_runtime,webdock_policy_owner;
GRANT SELECT ON webdock_auth.hosting_setup TO webdock_worker_runtime;
GRANT UPDATE(state,reason,resources,steps,generation,lease_until,next_attempt_at,updated_at) ON webdock_auth.hosting_setup TO webdock_worker_runtime;
CREATE POLICY setup_dispatch ON webdock_auth.hosting_setup TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['hosting-setup-dispatch'])) WITH CHECK(webdock_rls.worker(ARRAY['hosting-setup-dispatch']));
CREATE POLICY setup_execute_read ON webdock_auth.hosting_setup FOR SELECT TO webdock_worker_runtime USING(webdock_rls.setup_allowed(customer_id::text,project_id::text));
CREATE POLICY setup_user ON webdock_auth."user" FOR SELECT TO webdock_worker_runtime USING(webdock_rls.setup_identity(id::text));
CREATE POLICY setup_session ON webdock_auth.session FOR SELECT TO webdock_worker_runtime USING(webdock_rls.setup_identity("userId"::text,id::text));
CREATE POLICY setup_membership ON webdock_auth.member FOR SELECT TO webdock_worker_runtime USING(webdock_rls.setup_identity("userId"::text));
CREATE POLICY setup_preview ON webdock_auth.studio_tenant_preview FOR SELECT TO webdock_worker_runtime USING(webdock_rls.setup_identity(actor_id::text,session_id::text));
GRANT SELECT ON webdock_auth.studio_tenant_preview TO webdock_worker_runtime;
CREATE POLICY setup_customer_read ON webdock_admin.customers FOR SELECT TO webdock_worker_runtime USING(webdock_rls.setup_allowed(id::text));
CREATE POLICY setup_project_read ON webdock_admin.projects FOR SELECT TO webdock_worker_runtime USING(webdock_rls.setup_allowed(customer_id::text,id::text));
DO $$ DECLARE name text; BEGIN
 FOREACH name IN ARRAY ARRAY['hosting_project','hosting_service_binding','hosting_operation','hosting_reservation','hosting_database','hosting_database_offer','hosting_database_order','git_source','git_build','git_artifact','git_release'] LOOP
  EXECUTE format('GRANT SELECT,INSERT,UPDATE ON webdock_auth.%I TO webdock_worker_runtime',name);
  EXECUTE format('CREATE POLICY setup_execution ON webdock_auth.%I TO webdock_worker_runtime USING(webdock_rls.setup_allowed(customer_id::text,project_id::text)) WITH CHECK(webdock_rls.setup_allowed(customer_id::text,project_id::text))',name);
 END LOOP;
 FOREACH name IN ARRAY ARRAY['tenant_customer','tenant_subscription','hosting_limit','hosting_byok_policy','git_connection','git_repository','mail_connection','mail_connection_domain','mail_sender_domain'] LOOP
  EXECUTE format('GRANT SELECT ON webdock_auth.%I TO webdock_worker_runtime',name);
  EXECUTE format('CREATE POLICY setup_execution_read ON webdock_auth.%I FOR SELECT TO webdock_worker_runtime USING(webdock_rls.setup_allowed(customer_id::text))',name);
 END LOOP;
 FOREACH name IN ARRAY ARRAY['hosting_cluster','hosting_agent','hosting_database_backend'] LOOP
  EXECUTE format('GRANT SELECT ON webdock_auth.%I TO webdock_worker_runtime',name);
  EXECUTE format('CREATE POLICY setup_cluster_read ON webdock_auth.%I FOR SELECT TO webdock_worker_runtime USING(webdock_rls.setup_cluster(%I::text))',name,CASE WHEN name='hosting_cluster' THEN 'id' ELSE 'cluster_id' END);
 END LOOP;
END $$;
CREATE POLICY setup_app ON webdock_auth.hosting_app TO webdock_worker_runtime USING(EXISTS(SELECT 1 FROM webdock_auth.hosting_project p WHERE p.project_id=hosting_app.project_id AND webdock_rls.setup_allowed(p.customer_id::text,p.project_id::text))) WITH CHECK(EXISTS(SELECT 1 FROM webdock_auth.hosting_project p WHERE p.project_id=hosting_app.project_id AND webdock_rls.setup_allowed(p.customer_id::text,p.project_id::text)));
CREATE POLICY setup_settings ON webdock_auth.platform_settings FOR SELECT TO webdock_worker_runtime USING(webdock_rls.setup_allowed(webdock_rls.request_scope()->>'customerID'));
CREATE POLICY setup_plan_read ON webdock_auth.hosting_database_plan FOR SELECT TO webdock_worker_runtime USING(webdock_rls.setup_allowed(webdock_rls.request_scope()->>'customerID'));
CREATE POLICY setup_check ON webdock_auth.git_check_outbox TO webdock_worker_runtime USING(EXISTS(SELECT 1 FROM webdock_auth.git_build b WHERE b.id=git_check_outbox.build_id AND webdock_rls.setup_allowed(b.customer_id::text,b.project_id::text))) WITH CHECK(EXISTS(SELECT 1 FROM webdock_auth.git_build b WHERE b.id=git_check_outbox.build_id AND webdock_rls.setup_allowed(b.customer_id::text,b.project_id::text)));
CREATE POLICY setup_mail_key_read ON webdock_auth.mail_api_key FOR SELECT TO webdock_worker_runtime USING(webdock_rls.setup_allowed(customer_id::text));
CREATE POLICY setup_mail_key_insert ON webdock_auth.mail_api_key FOR INSERT TO webdock_worker_runtime WITH CHECK(permission='sending_access' AND EXISTS(SELECT 1 FROM webdock_auth.hosting_setup s WHERE s.id=webdock_rls.request_scope()->>'setupID' AND s.configuration->'mail'->>'connectionID'=mail_api_key.connection_id AND s.configuration->'mail'->>'domainID'=mail_api_key.domain_id AND webdock_rls.setup_allowed(s.customer_id::text,s.project_id::text)));
CREATE POLICY setup_mail_key_update ON webdock_auth.mail_api_key FOR UPDATE TO webdock_worker_runtime USING(EXISTS(SELECT 1 FROM webdock_auth.hosting_service_binding b WHERE b.mail_key_id=mail_api_key.id AND webdock_rls.setup_allowed(b.customer_id::text,b.project_id::text))) WITH CHECK(EXISTS(SELECT 1 FROM webdock_auth.hosting_service_binding b WHERE b.mail_key_id=mail_api_key.id AND webdock_rls.setup_allowed(b.customer_id::text,b.project_id::text)));
CREATE POLICY setup_audit ON webdock_auth.hosting_audit FOR INSERT TO webdock_worker_runtime WITH CHECK(webdock_rls.setup_identity(subject));
CREATE POLICY setup_mail_audit ON webdock_auth.access_event FOR INSERT TO webdock_worker_runtime WITH CHECK(webdock_rls.setup_identity(actor_id));
REVOKE CREATE ON SCHEMA webdock_rls FROM webdock_platform_owner;

CREATE OR REPLACE FUNCTION webdock_rls.customer_allowed(customer text, writing boolean DEFAULT false) RETURNS boolean LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE a jsonb;
BEGIN
 IF webdock_rls.worker(ARRAY['hosting-setup']) THEN RETURN webdock_rls.setup_allowed(customer); END IF;
 IF webdock_rls.worker(ARRAY['database-order']) THEN RETURN customer IS NOT NULL AND webdock_rls.request_scope()->>'customerID'=customer; END IF;
 a:=webdock_rls.actor(); IF a IS NULL THEN RETURN false; END IF;
 IF (a->>'operator')::boolean THEN RETURN true; END IF;
 IF a->'preview'<>'null'::jsonb THEN RETURN NOT writing AND coalesce((a->'preview'->>'valid')::boolean,false) AND a->'preview'->>'customer_id'=customer; END IF;
 RETURN EXISTS(SELECT 1 FROM webdock_auth.tenant_customer t JOIN webdock_admin.customers c ON c.id=t.customer_id JOIN webdock_auth.member m ON m."organizationId"=t.organization_id WHERE t.customer_id=customer AND c.status='active' AND m."userId"=a->>'subject' AND (NOT writing OR m.role IN ('owner','admin')));
END $$;

CREATE OR REPLACE FUNCTION webdock_rls.capacity_cluster_allowed(cluster_id text)
RETURNS boolean LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $fn$
DECLARE scope jsonb; cluster record;
BEGIN
 scope:=webdock_rls.request_scope();
 IF scope IS NULL OR cluster_id IS NULL THEN RETURN false; END IF;
 SELECT c.ownership,c.dedicated_customer_id INTO cluster FROM webdock_auth.hosting_cluster c WHERE c.id=capacity_cluster_allowed.cluster_id;
 IF NOT FOUND THEN RETURN false; END IF;
 IF scope->>'kind'='system' AND session_user='webdock_worker_runtime' THEN
  IF scope->>'clusterID' IS NOT NULL AND scope->>'clusterID'<>cluster_id THEN RETURN false; END IF;
  -- The authenticated build dispatcher publishes to both platform and BYOK clusters.
  IF scope->>'purpose'='hosting-setup' THEN RETURN webdock_rls.setup_cluster(cluster_id); END IF;
  IF scope->>'purpose'='git-worker' THEN RETURN true; END IF;
  IF scope->>'purpose'='agent' THEN RETURN coalesce(scope->>'clusterID'=cluster_id,false); END IF;
  IF scope->>'purpose'='database-order' THEN
   RETURN coalesce(scope->>'customerID' IS NOT NULL AND webdock_rls.customer_allowed(scope->>'customerID') AND
    (cluster.ownership='platform' OR (cluster.ownership='tenant' AND cluster.dedicated_customer_id=scope->>'customerID')),false);
  END IF;
  RETURN false;
 END IF;
 IF scope->>'kind' IS DISTINCT FROM 'session' OR session_user<>'webdock_auth_runtime' OR webdock_rls.actor() IS NULL THEN RETURN false; END IF;
 RETURN coalesce(webdock_rls.operator() OR cluster.ownership='platform' OR
  (cluster.ownership='tenant' AND webdock_rls.customer_allowed(cluster.dedicated_customer_id)),false);
END $fn$;
ALTER FUNCTION webdock_rls.capacity_cluster_allowed(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.capacity_cluster_allowed(text) FROM PUBLIC;
