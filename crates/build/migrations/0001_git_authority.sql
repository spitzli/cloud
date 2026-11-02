ALTER TABLE webdock_auth.git_build ADD COLUMN IF NOT EXISTS completion_hash text;
ALTER TABLE webdock_auth.git_artifact ADD COLUMN IF NOT EXISTS image_digest text CHECK(image_digest IS NULL OR image_digest~'^sha256:[a-f0-9]{64}$');
-- Native 0012: purpose-bound durable Git authority; no fabricated browser sessions.
ALTER TABLE webdock_auth.hosting_operation ADD COLUMN IF NOT EXISTS git_release_id text;
CREATE INDEX IF NOT EXISTS hosting_operation_git_release ON webdock_auth.hosting_operation(git_release_id) WHERE git_release_id IS NOT NULL;
CREATE OR REPLACE FUNCTION webdock_rls.git_actor_authority_internal(customer text,subject text,session_id text)
RETURNS jsonb LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE actor webdock_auth."user"%ROWTYPE; org text; operator boolean;
BEGIN
 SELECT * INTO actor FROM webdock_auth."user" u WHERE u.id=subject FOR SHARE;
 IF NOT FOUND OR NOT actor."emailVerified" OR actor."mustChangePassword" OR
 (coalesce(actor.banned,false) AND (actor."banExpires" IS NULL OR actor."banExpires">now())) OR
 NOT webdock_rls.native_identity_authority(subject) THEN RETURN NULL; END IF;
 operator := actor.role='operator' AND coalesce(actor."twoFactorEnabled",false) AND EXISTS(SELECT 1 FROM webdock_admin.users u WHERE u.auth_subject=subject AND u.role='operator');
 SELECT t.organization_id INTO org FROM webdock_auth.tenant_customer t JOIN webdock_admin.customers c ON c.id=t.customer_id WHERE c.id=customer AND c.status='active' FOR SHARE OF c,t;
 IF org IS NULL OR NOT webdock_rls.native_organization_authority(org) THEN RETURN NULL; END IF;
 IF NOT coalesce(operator,false) THEN
  PERFORM 1 FROM webdock_auth.member m WHERE m."organizationId"=org AND m."userId"=subject AND m.role IN ('owner','admin') FOR SHARE;
  IF NOT FOUND THEN RETURN NULL; END IF;
 END IF;
 IF session_id IS NOT NULL THEN
  PERFORM 1 FROM webdock_auth.session s WHERE s.id=session_id AND s."userId"=subject AND s."expiresAt">now() FOR SHARE;
  IF NOT FOUND THEN RETURN NULL; END IF;
 END IF;
 RETURN jsonb_build_object('subject',subject,'sessionID',session_id,'operator',coalesce(operator,false));
END $$;
CREATE OR REPLACE FUNCTION webdock_rls.git_source_policy_authority(source_id text,source_revision integer)
RETURNS jsonb LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE s webdock_auth.git_source%ROWTYPE;
BEGIN
 IF NOT webdock_rls.worker(ARRAY['git-worker']) THEN RETURN NULL; END IF;
 SELECT * INTO s FROM webdock_auth.git_source x WHERE x.id=source_id AND x.revision=source_revision AND x.enabled AND x.auto_publish AND x.policy_revision=x.revision FOR SHARE;
 IF NOT FOUND OR NOT EXISTS(SELECT 1 FROM webdock_auth.git_connection c WHERE c.id=s.connection_id AND c.customer_id=s.customer_id AND c.state='active') THEN RETURN NULL; END IF;
 RETURN webdock_rls.git_actor_authority_internal(s.customer_id,s.policy_subject,NULL);
END $$;
CREATE OR REPLACE FUNCTION webdock_rls.git_release_authority(release_id text)
RETURNS jsonb LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE r webdock_auth.git_release%ROWTYPE; s webdock_auth.git_source%ROWTYPE; actor jsonb;
BEGIN
 SELECT * INTO r FROM webdock_auth.git_release x WHERE x.id=release_id;
 IF NOT FOUND OR NOT (webdock_rls.worker(ARRAY['git-worker']) OR webdock_rls.agent_project(r.project_id)) THEN RETURN NULL; END IF;
 SELECT * INTO s FROM webdock_auth.git_source x WHERE x.id=r.source_id FOR SHARE;
 IF NOT FOUND OR NOT s.enabled OR s.recipe<>'dockerfile' OR s.desired_release_id IS DISTINCT FROM r.id OR s.revision<>r.source_revision OR s.environment_revision<>r.environment_revision OR s.environment_identity IS DISTINCT FROM r.environment_identity OR r.approved_by IS NULL OR NOT EXISTS(SELECT 1 FROM webdock_auth.git_connection c WHERE c.id=s.connection_id AND c.customer_id=s.customer_id AND c.state='active' AND c.generation=r.connection_generation) OR NOT EXISTS(SELECT 1 FROM webdock_auth.git_artifact a WHERE a.id=r.artifact_id AND a.retained AND a.customer_id=r.customer_id AND a.project_id=r.project_id) THEN RETURN NULL; END IF;
 IF r.approved_source='git-policy' THEN
  IF NOT s.auto_publish OR s.policy_revision<>s.revision OR s.policy_subject IS DISTINCT FROM r.approved_by THEN RETURN NULL; END IF;
  actor := webdock_rls.git_actor_authority_internal(r.customer_id,r.approved_by,NULL);
 ELSIF r.approved_source IN ('studio','oauth') AND r.approved_session_id IS NOT NULL AND r.approved_session_id<>'' THEN
  IF r.approved_source='oauth' AND NOT coalesce(r.approved_scopes @> '["hosting:write"]'::jsonb,false) THEN RETURN NULL; END IF;
  actor := webdock_rls.git_actor_authority_internal(r.customer_id,r.approved_by,r.approved_session_id);
 ELSE RETURN NULL;
 END IF;
 IF actor IS NULL OR NOT EXISTS(SELECT 1 FROM webdock_admin.projects p JOIN webdock_auth.hosting_project h ON h.project_id=p.id WHERE p.id=r.project_id AND p.customer_id=r.customer_id AND p.status='active' AND (h.mode='selfservice' OR (actor->>'operator')::boolean) AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=p.id AND d.status<>'deleted')) THEN RETURN NULL; END IF;
 RETURN actor || jsonb_build_object('customerID',r.customer_id,'projectID',r.project_id,'targetID',s.target_id);
END $$;
ALTER FUNCTION webdock_rls.git_actor_authority_internal(text,text,text) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.git_source_policy_authority(text,integer) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.git_release_authority(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.git_actor_authority_internal(text,text,text),webdock_rls.git_source_policy_authority(text,integer),webdock_rls.git_release_authority(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.git_source_policy_authority(text,integer),webdock_rls.git_release_authority(text) TO webdock_worker_runtime;
CREATE OR REPLACE FUNCTION webdock_rls.git_operation_authority(operation_id text)
RETURNS jsonb LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE release_id text;
BEGIN
 SELECT r.id INTO release_id FROM webdock_auth.hosting_operation o
 JOIN webdock_auth.git_release r ON r.id=o.git_release_id AND r.operation_id=o.id
 JOIN webdock_auth.git_source s ON s.id=r.source_id
 JOIN webdock_auth.git_artifact a ON a.id=r.artifact_id
 WHERE o.id=$1 AND o.project_id=r.project_id AND o.customer_id=r.customer_id
 AND o.app_id=s.target_id AND o.subject=r.approved_by
 AND o.action IN ('apps.deploy','apps.update')
 AND o.desired->'spec'->>'image'=(r.publication_target->>'imageRepository')||'@'||a.image_digest;
 IF release_id IS NULL THEN RETURN NULL; END IF;
 RETURN webdock_rls.git_release_authority(release_id);
END $$;
ALTER FUNCTION webdock_rls.git_operation_authority(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.git_operation_authority(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.git_operation_authority(text) TO webdock_worker_runtime;
