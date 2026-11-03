-- Existing-database metadata only. No provider key, URI, password or provider mutation.
CREATE TABLE webdock_auth.neon_database_attachment (
 resource_id text PRIMARY KEY,
 attach_invocation_id uuid NOT NULL UNIQUE,
 customer_id text NOT NULL REFERENCES webdock_admin.customers(id) ON DELETE CASCADE,
 project_id text NOT NULL,
 connection_id uuid NOT NULL REFERENCES webdock_auth.neon_connection(id),
 connection_revision bigint NOT NULL CHECK(connection_revision>0),
 revision bigint NOT NULL DEFAULT 1 CHECK(revision>0),
 name text NOT NULL CHECK(char_length(name) BETWEEN 1 AND 160),
 neon_project_id text NOT NULL CHECK(neon_project_id ~ '^[a-z0-9-]{1,63}$'),
 branch_id text NOT NULL CHECK(branch_id ~ '^[a-z0-9-]{1,63}$'),
 database_name text NOT NULL CHECK(octet_length(database_name) BETWEEN 1 AND 63),
 role_name text NOT NULL CHECK(octet_length(role_name) BETWEEN 1 AND 63),
 endpoint_id text NOT NULL CHECK(endpoint_id ~ '^[a-z0-9-]{1,63}$'),
 created_at timestamptz NOT NULL DEFAULT statement_timestamp(),
 detached_at timestamptz
);
CREATE TABLE webdock_auth.neon_database_receipt (
 customer_id text NOT NULL REFERENCES webdock_admin.customers(id) ON DELETE CASCADE,
 project_id text NOT NULL,
 actor_id text NOT NULL,
 operation_id text NOT NULL CHECK(operation_id ~ '^[a-f0-9]{64}$'),
 action text NOT NULL CHECK(action IN ('databases.attach','databases.detach')),
 request_hash text NOT NULL CHECK(request_hash ~ '^[a-f0-9]{64}$'),
 resource_id text NOT NULL REFERENCES webdock_auth.neon_database_attachment(resource_id),
 result jsonb NOT NULL,
 created_at timestamptz NOT NULL DEFAULT statement_timestamp(),
 PRIMARY KEY(customer_id,project_id,actor_id,operation_id)
);
ALTER TABLE webdock_auth.neon_database_attachment OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.neon_database_receipt OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.neon_database_attachment ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.neon_database_attachment FORCE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.neon_database_receipt ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.neon_database_receipt FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_auth.neon_database_attachment,webdock_auth.neon_database_receipt FROM PUBLIC,webdock_auth_bootstrap,webdock_auth_runtime,webdock_worker_runtime,webdock_registry_runtime;
GRANT SELECT ON webdock_auth.neon_database_attachment,webdock_auth.neon_database_receipt TO webdock_auth_runtime;
GRANT SELECT ON webdock_auth.neon_database_receipt TO webdock_worker_runtime;
CREATE POLICY platform_owner ON webdock_auth.neon_database_attachment TO webdock_platform_owner USING(true) WITH CHECK(true);
CREATE POLICY platform_owner ON webdock_auth.neon_database_receipt TO webdock_platform_owner USING(true) WITH CHECK(true);
CREATE POLICY attachment_read ON webdock_auth.neon_database_attachment FOR SELECT TO webdock_auth_runtime USING(webdock_rls.neon_connection_allowed(customer_id,false) AND webdock_rls.customer_allowed(customer_id,false));
CREATE POLICY receipt_read ON webdock_auth.neon_database_receipt FOR SELECT TO webdock_auth_runtime USING(webdock_rls.neon_connection_allowed(customer_id,true) AND actor_id=webdock_rls.subject());
CREATE POLICY controller_receipt ON webdock_auth.neon_database_receipt FOR SELECT TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['plugin-controller']::text[]));
CREATE FUNCTION webdock_auth.neon_database_receipt_immutable() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$ BEGIN
 IF TG_OP='DELETE' AND NOT EXISTS(SELECT 1 FROM webdock_admin.customers WHERE id=OLD.customer_id) THEN RETURN OLD; END IF;
 RAISE EXCEPTION 'Database attachment receipts are immutable' USING ERRCODE='55000';
END $$;
ALTER FUNCTION webdock_auth.neon_database_receipt_immutable() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.neon_database_receipt_immutable() FROM PUBLIC;
CREATE TRIGGER neon_database_receipt_immutable BEFORE UPDATE OR DELETE ON webdock_auth.neon_database_receipt FOR EACH ROW EXECUTE FUNCTION webdock_auth.neon_database_receipt_immutable();
-- Narrow local transaction: current actor, exact persisted action and reservation own every field.
GRANT CREATE ON SCHEMA webdock_rls TO webdock_platform_owner;
CREATE FUNCTION webdock_rls.commit_neon_database(invocation uuid, connection uuid, expected_connection_revision bigint, expected_attachment_revision bigint, input jsonb) RETURNS jsonb
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE v webdock_auth.plugin_invocation%ROWTYPE; c webdock_auth.neon_connection%ROWTYPE;
 b webdock_auth.neon_database_attachment%ROWTYPE; receipt webdock_auth.neon_database_receipt%ROWTYPE;
 resource text; result jsonb;
BEGIN
 PERFORM pg_advisory_xact_lock_shared(hashtext('webdock-plugin-installation'));
 SELECT * INTO v FROM webdock_auth.plugin_invocation WHERE id=invocation AND kind='action' AND plugin_id='neon'
  AND name IN ('databases.attach','databases.detach') AND intent='write' AND capabilities ? 'plugin.http.request'
  AND actor_id=webdock_rls.subject() AND session_id=webdock_rls.actor()->>'sessionID'
  AND session_context #>> '{authority,source}'='browser'
  AND revoked_at IS NULL AND expires_at>clock_timestamp();
 IF NOT FOUND OR NOT webdock_rls.neon_connection_allowed(v.customer_id,true)
  OR NOT EXISTS(SELECT 1 FROM webdock_admin.projects p JOIN webdock_admin.customers customer_row ON customer_row.id=p.customer_id
    WHERE p.id=v.project_id AND p.customer_id=v.customer_id AND p.status='active' AND customer_row.status='active'
    AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=p.id AND d.status<>'deleted')) THEN
  RAISE EXCEPTION 'Current browser invocation required' USING ERRCODE='42501';
 END IF;
 PERFORM 1 FROM webdock_auth.plugin_grant g JOIN webdock_auth.plugin_installation i ON i.id=g.plugin_id
  JOIN webdock_auth.plugin_runtime rt ON rt.plugin_id=i.id AND rt.generation=i.current_generation
  JOIN webdock_auth.plugin_configuration cfg ON cfg.plugin_id=i.id
  JOIN webdock_auth.plugin_release rel ON rel.plugin_id=i.id AND rel.version=i.version
  WHERE g.plugin_id=v.plugin_id AND g.customer_id=v.customer_id AND g.project_id=v.project_id AND g.revoked_at IS NULL
   AND g.revision=v.grant_revision AND g.capabilities ? 'plugin.http.request'
   AND i.state='enabled' AND i.runtime='ready' AND i.current_generation=v.runtime_generation AND rt.status='ready'
   AND cfg.revision=(v.session_context #>> '{nativeAdmission,configurationRevision}')::bigint
   AND rt.credential_hash=v.session_context #>> '{nativeAdmission,runtimeCredentialSHA256}'
   AND rel.manifest_sha256=v.session_context #>> '{nativeAdmission,manifestSHA256}'
   AND cfg.approved_capabilities ? 'plugin.http.request'
  FOR SHARE OF g,i,rt,cfg,rel;
 IF NOT FOUND THEN RAISE EXCEPTION 'Current plugin grant required' USING ERRCODE='42501'; END IF;
 PERFORM 1 FROM webdock_auth.plugin_invocation WHERE id=v.id AND revoked_at IS NULL AND expires_at>clock_timestamp() FOR SHARE;
 IF NOT FOUND OR input IS DISTINCT FROM v.session_context->'neonAttachmentInput' THEN RAISE EXCEPTION 'Admitted input required' USING ERRCODE='42501'; END IF;
 IF v.name='databases.attach' AND (NOT webdock_rls.plugin_feature_supported('neon','connect-databases') OR
  NOT webdock_rls.operator() AND NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_feature_policy p WHERE p.plugin_id='neon' AND p.feature_id='connect-databases'
   AND (p.mode='everyone' OR p.mode='selected' AND EXISTS(SELECT 1 FROM webdock_auth.plugin_feature_customer f WHERE f.plugin_id=p.plugin_id AND f.feature_id=p.feature_id AND f.customer_id=v.customer_id)))) THEN
  RAISE EXCEPTION 'Current database feature required' USING ERRCODE='42501';
 END IF;
 PERFORM pg_advisory_xact_lock(hashtextextended('native-neon-connection:'||v.customer_id,0));
 PERFORM pg_advisory_xact_lock(hashtext('webdock-limits:'||v.customer_id));
 SELECT * INTO receipt FROM webdock_auth.neon_database_receipt WHERE customer_id=v.customer_id AND project_id=v.project_id
  AND actor_id=v.actor_id AND operation_id=v.operation_id;
 IF FOUND THEN
  IF receipt.action<>v.name OR receipt.request_hash<>v.request_hash THEN RAISE EXCEPTION 'Database operation conflict' USING ERRCODE='23514'; END IF;
  RETURN receipt.result;
 END IF;
 SELECT * INTO c FROM webdock_auth.neon_connection WHERE customer_id=v.customer_id AND id=connection AND revision=expected_connection_revision FOR SHARE;
 IF NOT FOUND THEN RAISE EXCEPTION 'Connection changed' USING ERRCODE='23514'; END IF;
 IF v.name='databases.attach' THEN
  IF c.status<>'connected' OR v.session_context #>> '{resourceAction,effect}' IS DISTINCT FROM 'create'
   OR v.session_context #>> '{resourceAction,offering}' IS DISTINCT FROM 'database'
   OR v.session_context #>> '{resourceCreate,invocationID}' IS DISTINCT FROM v.id::text
   OR v.session_context #>> '{resourceCreate,state}' IS DISTINCT FROM 'pending'
   OR c.neon_project_id IS NOT NULL AND c.neon_project_id IS DISTINCT FROM input->>'neon_project_id'
   OR EXISTS(SELECT 1 FROM webdock_auth.plugin_job_execution WHERE provenance #>> '{session_context,resourceCreate,invocationID}'=v.id::text)
   THEN RAISE EXCEPTION 'Database reservation required' USING ERRCODE='23514'; END IF;
  resource:='neon-'||gen_random_uuid()::text;
  INSERT INTO webdock_auth.neon_database_attachment(resource_id,attach_invocation_id,customer_id,project_id,connection_id,connection_revision,name,neon_project_id,branch_id,database_name,role_name,endpoint_id)
   VALUES(resource,v.id,v.customer_id,v.project_id,c.id,c.revision,input->>'name',input->>'neon_project_id',input->>'branch_id',input->>'database_name',input->>'role_name',input->>'endpoint_id');
  INSERT INTO webdock_auth.plugin_resource(plugin_id,resource_id,offering_id,customer_id,project_id,name,created_via,created_by)
   VALUES('neon',resource,'database',v.customer_id,v.project_id,input->>'name',CASE WHEN v.session_context->'operator'='true'::jsonb THEN 'operator' ELSE 'self-service' END,v.actor_id);
  UPDATE webdock_auth.plugin_invocation SET session_context=jsonb_set(session_context,'{resourceCreate}',session_context->'resourceCreate'||jsonb_build_object('state','indexed','resourceID',resource)) WHERE id=v.id;
  result:=jsonb_build_object('resource',jsonb_build_object('id',resource,'name',input->>'name'));
 ELSE
  resource:=v.session_context->>'resourceID';
  SELECT * INTO b FROM webdock_auth.neon_database_attachment WHERE resource_id=resource AND customer_id=v.customer_id AND project_id=v.project_id AND detached_at IS NULL AND revision=expected_attachment_revision FOR UPDATE;
  IF NOT FOUND OR v.session_context #>> '{resourceAction,effect}' IS DISTINCT FROM 'delete' OR input->>'resource_id' IS DISTINCT FROM resource
   OR NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_resource WHERE plugin_id='neon' AND resource_id=resource AND customer_id=v.customer_id AND project_id=v.project_id)
   THEN RAISE EXCEPTION 'Current database attachment required' USING ERRCODE='23514'; END IF;
  UPDATE webdock_auth.neon_database_attachment SET detached_at=statement_timestamp(),revision=revision+1 WHERE resource_id=resource;
  DELETE FROM webdock_auth.plugin_resource WHERE plugin_id='neon' AND resource_id=resource AND customer_id=v.customer_id AND project_id=v.project_id;
  result:=jsonb_build_object('detached',true,'resource_id',resource);
 END IF;
 INSERT INTO webdock_auth.neon_database_receipt(customer_id,project_id,actor_id,operation_id,action,request_hash,resource_id,result)
 VALUES(v.customer_id,v.project_id,v.actor_id,v.operation_id,v.name,v.request_hash,resource,result);
 RETURN result;
END $$;
ALTER FUNCTION webdock_rls.commit_neon_database(uuid,uuid,bigint,bigint,jsonb) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.commit_neon_database(uuid,uuid,bigint,bigint,jsonb) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.commit_neon_database(uuid,uuid,bigint,bigint,jsonb) TO webdock_auth_runtime;
REVOKE CREATE ON SCHEMA webdock_rls FROM webdock_platform_owner;

-- Local attach never creates provider resources: only an uncommitted, dead invocation is reclaimable.
GRANT CREATE ON SCHEMA webdock_rls TO webdock_platform_owner;
CREATE FUNCTION webdock_rls.reclaim_neon_attachment_reservation(invocation uuid) RETURNS void
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE v webdock_auth.plugin_invocation%ROWTYPE; prior webdock_auth.plugin_invocation%ROWTYPE;
BEGIN
 IF NOT webdock_rls.worker(ARRAY['plugin-controller']::text[]) THEN RAISE EXCEPTION 'Plugin controller required' USING ERRCODE='42501'; END IF;
 SELECT * INTO v FROM webdock_auth.plugin_invocation WHERE id=invocation AND plugin_id='neon' AND kind='action'
  AND name='databases.attach' AND intent='write' AND revoked_at IS NULL AND expires_at>clock_timestamp();
 IF NOT FOUND THEN RAISE EXCEPTION 'Current Neon attach required' USING ERRCODE='42501'; END IF;
 -- Same order as the native Connections commit: no transaction holds these over provider IO.
 PERFORM pg_advisory_xact_lock(hashtextextended('native-neon-connection:'||v.customer_id,0));
 PERFORM pg_advisory_xact_lock(hashtext('webdock-limits:'||v.customer_id));
 SELECT * INTO prior FROM webdock_auth.plugin_invocation p WHERE p.id<>v.id AND p.plugin_id=v.plugin_id
  AND p.customer_id=v.customer_id AND p.project_id=v.project_id AND p.actor_id=v.actor_id
  AND p.name=v.name AND p.kind='action' AND p.operation_id=v.operation_id
  AND p.session_context #>> '{resourceCreate,invocationID}'=p.id::text FOR UPDATE;
 IF NOT FOUND OR prior.session_context #>> '{resourceCreate,state}' IS DISTINCT FROM 'pending'
  OR prior.revoked_at IS NULL AND prior.expires_at>clock_timestamp() THEN RETURN; END IF;
 IF prior.request_hash IS DISTINCT FROM v.request_hash OR
  EXISTS(SELECT 1 FROM webdock_auth.neon_database_receipt r WHERE r.customer_id=v.customer_id AND r.project_id=v.project_id AND r.actor_id=v.actor_id AND r.operation_id=v.operation_id) OR
  EXISTS(SELECT 1 FROM webdock_auth.neon_database_attachment b WHERE b.attach_invocation_id=prior.id) OR
  EXISTS(SELECT 1 FROM webdock_auth.plugin_resource r WHERE r.plugin_id='neon' AND r.customer_id=v.customer_id AND r.project_id=v.project_id AND r.resource_id=prior.session_context #>> '{resourceCreate,resourceID}') THEN
  RAISE EXCEPTION 'Attachment reservation conflicts with retained metadata' USING ERRCODE='23514';
 END IF;
 UPDATE webdock_auth.plugin_invocation SET revoked_at=coalesce(revoked_at,statement_timestamp()),
  session_context=(session_context-'resourceCreate')||jsonb_build_object('neonAttachAbandoned',jsonb_build_object('reason','uncommitted-local-attachment','invocationID',prior.id)) WHERE id=prior.id;
END $$;
ALTER FUNCTION webdock_rls.reclaim_neon_attachment_reservation(uuid) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.reclaim_neon_attachment_reservation(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.reclaim_neon_attachment_reservation(uuid) TO webdock_worker_runtime;
REVOKE CREATE ON SCHEMA webdock_rls FROM webdock_platform_owner;
