CREATE TABLE IF NOT EXISTS webdock_admin.project_deletion (
 project_id varchar PRIMARY KEY, plan jsonb NOT NULL, plan_hash text NOT NULL,
 completed_steps jsonb NOT NULL DEFAULT '[]', status text NOT NULL DEFAULT 'prepared',
 actor_id text NOT NULL, last_error text, updated_at timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE webdock_admin.project_deletion OWNER TO webdock_platform_owner;
ALTER TABLE webdock_admin.project_deletion ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_admin.project_deletion FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_admin.project_deletion FROM PUBLIC;
GRANT SELECT,INSERT,UPDATE,DELETE ON webdock_admin.project_deletion TO webdock_registry_runtime;
CREATE POLICY api_registry_deletion ON webdock_admin.project_deletion TO webdock_registry_runtime USING (webdock_rls.operator()) WITH CHECK (webdock_rls.operator());
ALTER TABLE webdock_admin.project_deletion ADD COLUMN claim_token uuid;
ALTER TABLE webdock_admin.project_deletion ADD COLUMN lease_until timestamptz;
CREATE OR REPLACE FUNCTION webdock_admin.fence_deleting_registry() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_admin AS $$
DECLARE target text; held record; supplied text;
BEGIN
 IF session_user NOT IN ('webdock_registry_runtime','webdock_admin_runtime','webdock_auth_runtime') THEN
  IF TG_OP='DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
 END IF;
 IF TG_TABLE_NAME='projects' THEN target:=CASE WHEN TG_OP='DELETE' THEN OLD.id ELSE NEW.id END;
 ELSE target:=CASE WHEN TG_OP='DELETE' THEN OLD.project_id ELSE NEW.project_id END; END IF;
 FOR held IN SELECT j.project_id,j.claim_token,j.lease_until FROM webdock_admin.project_deletion j WHERE j.status<>'deleted' AND
  (j.project_id=target OR (TG_TABLE_NAME='vercel_project_link' AND TG_OP<>'DELETE' AND to_jsonb(NEW)->>'team_id'=j.plan->>'teamID' AND to_jsonb(NEW)->>'vercel_project_id'=j.plan->>'providerProjectID') OR (TG_TABLE_NAME='cms_instances' AND TG_OP<>'DELETE' AND EXISTS(
    SELECT 1 FROM webdock_admin.cms_instances old_instance WHERE old_instance.project_id=j.project_id AND
    (to_jsonb(NEW)->>'schema_name'=old_instance.schema_name OR to_jsonb(NEW)->>'provider_project_i_d'=old_instance.provider_project_i_d OR to_jsonb(NEW)->>'admin_u_r_l'=old_instance.admin_u_r_l)
  )))
 LOOP
  supplied:=current_setting('webdock.deletion_token',true);
  IF supplied IS DISTINCT FROM held.claim_token::text OR held.lease_until IS NULL OR held.lease_until<=statement_timestamp() OR NOT webdock_rls.operator() THEN
   RAISE EXCEPTION 'Project deletion is in progress; registry changes are fenced' USING ERRCODE='55000';
  END IF;
 END LOOP;
 IF TG_OP='DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END $$;
ALTER FUNCTION webdock_admin.fence_deleting_registry() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_admin.fence_deleting_registry() FROM PUBLIC;
CREATE TRIGGER api_deletion_project_fence BEFORE UPDATE OR DELETE ON webdock_admin.projects FOR EACH ROW EXECUTE FUNCTION webdock_admin.fence_deleting_registry();
CREATE TRIGGER api_deletion_instance_fence BEFORE INSERT OR UPDATE OR DELETE ON webdock_admin.cms_instances FOR EACH ROW EXECUTE FUNCTION webdock_admin.fence_deleting_registry();

CREATE TRIGGER api_deletion_vercel_fence BEFORE INSERT OR UPDATE OR DELETE ON webdock_admin.vercel_project_link FOR EACH ROW EXECUTE FUNCTION webdock_admin.fence_deleting_registry();
