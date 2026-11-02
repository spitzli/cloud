-- DRAFT ONLY: outside SQLx migrations. Do not install before Root freezes Mail0052.
-- Forward0053: admit explicit retained frontend outputs without repurposing vercel.
-- All existing git_release_authority predicates remain unchanged except its recipe allowlist.
ALTER TABLE webdock_auth.git_source DROP CONSTRAINT git_source_recipe_check;
ALTER TABLE webdock_auth.git_source ADD CONSTRAINT git_source_recipe_check
 CHECK (recipe IN ('dockerfile','vercel','dockerfile-frontend'));

CREATE OR REPLACE FUNCTION webdock_rls.git_release_authority(release_id text)
RETURNS jsonb LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE r webdock_auth.git_release%ROWTYPE; s webdock_auth.git_source%ROWTYPE; actor jsonb;
BEGIN
 SELECT * INTO r FROM webdock_auth.git_release x WHERE x.id=release_id;
 IF NOT FOUND OR NOT (webdock_rls.worker(ARRAY['git-worker']) OR webdock_rls.agent_project(r.project_id)) THEN RETURN NULL; END IF;
 SELECT * INTO s FROM webdock_auth.git_source x WHERE x.id=r.source_id FOR SHARE;
 IF NOT FOUND OR NOT s.enabled OR s.recipe NOT IN ('dockerfile','dockerfile-frontend') OR s.desired_release_id IS DISTINCT FROM r.id OR s.revision<>r.source_revision OR s.environment_revision<>r.environment_revision OR s.environment_identity IS DISTINCT FROM r.environment_identity OR r.approved_by IS NULL OR NOT EXISTS(SELECT 1 FROM webdock_auth.git_connection c WHERE c.id=s.connection_id AND c.customer_id=s.customer_id AND c.state='active' AND c.generation=r.connection_generation) OR NOT EXISTS(SELECT 1 FROM webdock_auth.git_artifact a WHERE a.id=r.artifact_id AND a.retained AND a.customer_id=r.customer_id AND a.project_id=r.project_id) THEN RETURN NULL; END IF;
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
ALTER FUNCTION webdock_rls.git_release_authority(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.git_release_authority(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.git_release_authority(text) TO webdock_worker_runtime;
