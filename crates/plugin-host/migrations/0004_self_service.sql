-- Native0026: per-feature self-service modes, per-customer allow lists and the Core resource index.
-- A missing policy row means 'assigned': nothing is self-service until an operator decides.
ALTER TABLE webdock_auth.plugin_installation ADD COLUMN policy_revision bigint NOT NULL DEFAULT 0 CHECK (policy_revision >= 0);

CREATE TABLE webdock_auth.plugin_feature_policy (
 plugin_id text NOT NULL REFERENCES webdock_auth.plugin_installation(id) ON DELETE CASCADE,
 feature_id text NOT NULL CHECK (feature_id ~ '^[a-z][a-z0-9-]{0,63}$'),
 mode text NOT NULL CHECK (mode IN ('everyone','selected','assigned')),
 updated_by text NOT NULL,
 updated_at timestamptz NOT NULL DEFAULT now(),
 PRIMARY KEY (plugin_id, feature_id)
);
CREATE TABLE webdock_auth.plugin_feature_customer (
 plugin_id text NOT NULL REFERENCES webdock_auth.plugin_installation(id) ON DELETE CASCADE,
 feature_id text NOT NULL CHECK (feature_id ~ '^[a-z][a-z0-9-]{0,63}$'),
 customer_id text NOT NULL REFERENCES webdock_admin.customers(id) ON DELETE CASCADE,
 added_by text NOT NULL,
 added_at timestamptz NOT NULL DEFAULT now(),
 PRIMARY KEY (plugin_id, feature_id, customer_id)
);
-- Resources block project (and so customer) deletion: no cascade.
CREATE TABLE webdock_auth.plugin_resource (
 plugin_id text NOT NULL REFERENCES webdock_auth.plugin_installation(id),
 resource_id text NOT NULL CHECK (resource_id ~ '^[A-Za-z0-9_.:-]{1,128}$'),
 offering_id text NOT NULL CHECK (offering_id ~ '^[a-z][a-z0-9-]{0,63}$'),
 customer_id text NOT NULL,
 project_id text NOT NULL REFERENCES webdock_admin.projects(id),
 name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 160),
 created_via text NOT NULL CHECK (created_via IN ('operator','self-service')),
 created_by text NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now(),
 PRIMARY KEY (plugin_id, resource_id)
);
CREATE INDEX plugin_resource_project ON webdock_auth.plugin_resource(project_id, plugin_id);

DO $$
DECLARE t text;
BEGIN
 FOREACH t IN ARRAY ARRAY['plugin_feature_policy','plugin_feature_customer','plugin_resource'] LOOP
  EXECUTE format('ALTER TABLE webdock_auth.%I OWNER TO webdock_platform_owner', t);
  EXECUTE format('ALTER TABLE webdock_auth.%I ENABLE ROW LEVEL SECURITY', t);
  EXECUTE format('ALTER TABLE webdock_auth.%I FORCE ROW LEVEL SECURITY', t);
  EXECUTE format('CREATE POLICY platform_owner ON webdock_auth.%I TO webdock_platform_owner USING (true) WITH CHECK (true)', t);
  EXECUTE format('CREATE POLICY webdock_worker_runtime_read ON webdock_auth.%I FOR SELECT TO webdock_worker_runtime USING (webdock_rls.worker(ARRAY[''plugin-controller'']::text[]))', t);
  EXECUTE format('GRANT SELECT ON TABLE webdock_auth.%I TO webdock_worker_runtime', t);
 END LOOP;
END $$;
-- Policies are installation-wide and only operators change them. Customers read their own allow-list rows.
CREATE POLICY webdock_auth_runtime_read ON webdock_auth.plugin_feature_policy FOR SELECT TO webdock_auth_runtime USING (webdock_rls.operator());
CREATE POLICY webdock_auth_runtime_insert ON webdock_auth.plugin_feature_policy FOR INSERT TO webdock_auth_runtime WITH CHECK (webdock_rls.operator());
CREATE POLICY webdock_auth_runtime_update ON webdock_auth.plugin_feature_policy FOR UPDATE TO webdock_auth_runtime USING (webdock_rls.operator()) WITH CHECK (webdock_rls.operator());
CREATE POLICY webdock_auth_runtime_delete ON webdock_auth.plugin_feature_policy FOR DELETE TO webdock_auth_runtime USING (webdock_rls.operator());
CREATE POLICY webdock_auth_runtime_read ON webdock_auth.plugin_feature_customer FOR SELECT TO webdock_auth_runtime USING (webdock_rls.operator() OR webdock_rls.customer_allowed(customer_id));
CREATE POLICY webdock_auth_runtime_insert ON webdock_auth.plugin_feature_customer FOR INSERT TO webdock_auth_runtime WITH CHECK (webdock_rls.operator());
CREATE POLICY webdock_auth_runtime_delete ON webdock_auth.plugin_feature_customer FOR DELETE TO webdock_auth_runtime USING (webdock_rls.operator());
CREATE POLICY webdock_auth_runtime_read ON webdock_auth.plugin_resource FOR SELECT TO webdock_auth_runtime USING (webdock_rls.operator() OR webdock_rls.customer_allowed(customer_id));
GRANT SELECT,INSERT,UPDATE,DELETE ON TABLE webdock_auth.plugin_feature_policy TO webdock_auth_runtime;
GRANT SELECT,INSERT,DELETE ON TABLE webdock_auth.plugin_feature_customer TO webdock_auth_runtime;
GRANT SELECT ON TABLE webdock_auth.plugin_resource TO webdock_auth_runtime;

-- Grants become an admission cache: one row per project the plugin is used in, carrying the operator's
-- capability ceiling and the installation's policy revision. Only the Core plugin controller fills it.
CREATE FUNCTION webdock_rls.ensure_plugin_grant(plugin text, project text) RETURNS void
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 IF NOT webdock_rls.worker(ARRAY['plugin-controller']::text[]) THEN RAISE EXCEPTION 'plugin controller required' USING ERRCODE='42501'; END IF;
 INSERT INTO webdock_auth.plugin_grant(plugin_id,customer_id,project_id,capabilities,revision)
 SELECT p.id,project_row.customer_id,project_row.id,c.approved_capabilities,p.policy_revision
 FROM webdock_auth.plugin_installation p
 JOIN webdock_auth.plugin_configuration c ON c.plugin_id=p.id
 JOIN webdock_admin.projects project_row ON project_row.id=project
 WHERE p.id=plugin AND p.kind='external'
 ON CONFLICT (plugin_id,customer_id,project_id) DO UPDATE SET capabilities=EXCLUDED.capabilities,revision=EXCLUDED.revision,revoked_at=NULL
  WHERE webdock_auth.plugin_grant.revision<EXCLUDED.revision;
END $$;
ALTER FUNCTION webdock_rls.ensure_plugin_grant(text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.ensure_plugin_grant(text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.ensure_plugin_grant(text,text) TO webdock_worker_runtime;

-- Any policy or capability change bumps the revision; invocations admitted before it fail revalidation.
CREATE FUNCTION webdock_rls.bump_plugin_policy(plugin text) RETURNS bigint
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE new_revision bigint;
BEGIN
 IF NOT webdock_rls.operator() THEN RAISE EXCEPTION 'operator required' USING ERRCODE='42501'; END IF;
 UPDATE webdock_auth.plugin_installation SET policy_revision=policy_revision+1 WHERE id=plugin AND kind='external' RETURNING policy_revision INTO new_revision;
 IF new_revision IS NULL THEN RAISE EXCEPTION 'unknown plugin' USING ERRCODE='P0002'; END IF;
 UPDATE webdock_auth.plugin_grant g SET revision=new_revision,capabilities=c.approved_capabilities
 FROM webdock_auth.plugin_configuration c WHERE g.plugin_id=plugin AND c.plugin_id=plugin;
 RETURN new_revision;
END $$;
ALTER FUNCTION webdock_rls.bump_plugin_policy(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.bump_plugin_policy(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.bump_plugin_policy(text) TO webdock_auth_runtime;

-- Record or remove the resource named by a finished create/delete invocation. Customer, project and
-- offering come from the persisted invocation and its release manifest, never from plugin output.
CREATE FUNCTION webdock_rls.record_plugin_resource(invocation uuid, resource text, resource_name text, by_operator boolean) RETURNS boolean
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE v webdock_auth.plugin_invocation%ROWTYPE; action jsonb;
BEGIN
 IF NOT webdock_rls.worker(ARRAY['plugin-controller']::text[]) THEN RAISE EXCEPTION 'plugin controller required' USING ERRCODE='42501'; END IF;
 SELECT * INTO v FROM webdock_auth.plugin_invocation WHERE id=invocation AND kind='action';
 IF NOT FOUND THEN RETURN false; END IF;
 SELECT a.value INTO action FROM webdock_auth.plugin_installation p
 JOIN webdock_auth.plugin_release r ON r.plugin_id=p.id AND r.version=p.version,
 jsonb_array_elements(r.manifest_bytes::jsonb->'actions') a
 WHERE p.id=v.plugin_id AND a.value->>'name'=v.name;
 IF action IS NULL THEN RETURN false; END IF;
 IF action->>'resource'='create' THEN
  INSERT INTO webdock_auth.plugin_resource(plugin_id,resource_id,offering_id,customer_id,project_id,name,created_via,created_by)
  VALUES(v.plugin_id,resource,action->>'offering',v.customer_id,v.project_id,resource_name,CASE WHEN by_operator THEN 'operator' ELSE 'self-service' END,v.actor_id)
  ON CONFLICT (plugin_id,resource_id) DO NOTHING;
  RETURN EXISTS(SELECT 1 FROM webdock_auth.plugin_resource WHERE plugin_id=v.plugin_id AND resource_id=resource AND project_id=v.project_id);
 ELSIF action->>'resource'='delete' THEN
  DELETE FROM webdock_auth.plugin_resource WHERE plugin_id=v.plugin_id AND resource_id=resource AND project_id=v.project_id;
  RETURN true;
 END IF;
 RETURN false;
END $$;
ALTER FUNCTION webdock_rls.record_plugin_resource(uuid,text,text,boolean) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.record_plugin_resource(uuid,text,text,boolean) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.record_plugin_resource(uuid,text,text,boolean) TO webdock_worker_runtime;
