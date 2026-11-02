-- Retained runtime generations are immutable custody, not a replaceable installation slot.
ALTER TABLE webdock_auth.plugin_install_execution DROP CONSTRAINT plugin_install_execution_pkey,
 ADD PRIMARY KEY(plugin_id,runtime_generation),
 ADD COLUMN origin_operation_id text REFERENCES webdock_auth.plugin_operation(id),
 ADD COLUMN source_runtime_generation bigint,
 ADD COLUMN configuration_snapshot jsonb,
 ADD COLUMN quiesce_generations jsonb NOT NULL DEFAULT '[]' CHECK(jsonb_typeof(quiesce_generations)='array'),
 ADD COLUMN quiesce_index integer NOT NULL DEFAULT 0 CHECK(quiesce_index>=0),
 ADD COLUMN apply_dispatched boolean NOT NULL DEFAULT false,
 ADD COLUMN stopped boolean NOT NULL DEFAULT false,
 ADD COLUMN promoted_at timestamptz;
UPDATE webdock_auth.plugin_install_execution e SET origin_operation_id=e.operation_id,
 configuration_snapshot=jsonb_build_object('configuration',c.configuration,'approvedCapabilities',c.approved_capabilities),
 apply_dispatched=EXISTS(SELECT 1 FROM webdock_auth.plugin_install_dispatch d WHERE d.spec_sha256=e.spec_sha256),
 promoted_at=CASE WHEN p.current_generation=e.runtime_generation THEN r.observed_at END
 FROM webdock_auth.plugin_configuration c,webdock_auth.plugin_installation p,webdock_auth.plugin_runtime r
 WHERE c.plugin_id=e.plugin_id AND p.id=e.plugin_id AND r.plugin_id=e.plugin_id AND r.generation=e.runtime_generation;
ALTER TABLE webdock_auth.plugin_install_execution ALTER COLUMN origin_operation_id SET NOT NULL,
 ALTER COLUMN configuration_snapshot SET NOT NULL,
 ADD FOREIGN KEY(plugin_id,source_runtime_generation) REFERENCES webdock_auth.plugin_runtime(plugin_id,generation);
CREATE UNIQUE INDEX plugin_install_execution_operation ON webdock_auth.plugin_install_execution(operation_id);
ALTER TABLE webdock_auth.plugin_installation ADD COLUMN schema_high_water bigint CHECK(schema_high_water>0);
CREATE FUNCTION webdock_auth.guard_plugin_schema_high_water() RETURNS trigger
 LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 IF OLD.schema_high_water IS NOT NULL AND
   (NEW.schema_high_water IS NULL OR NEW.schema_high_water<OLD.schema_high_water) THEN
  RAISE EXCEPTION 'Retained logical schema cannot decrease' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_auth.guard_plugin_schema_high_water() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.guard_plugin_schema_high_water() FROM PUBLIC;
CREATE TRIGGER guard_plugin_schema_high_water BEFORE UPDATE OF schema_high_water
 ON webdock_auth.plugin_installation FOR EACH ROW EXECUTE FUNCTION webdock_auth.guard_plugin_schema_high_water();
-- Existing baselines remain unknown until verified against loaded signed initial provenance.
CREATE OR REPLACE FUNCTION webdock_rls.plugin_feature_supported(plugin text, feature text) RETURNS boolean
 LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
 SELECT NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_install_execution e WHERE e.plugin_id=plugin)
 OR EXISTS(SELECT 1 FROM webdock_auth.plugin_install_execution e JOIN webdock_auth.plugin_installation p
 ON p.id=e.plugin_id AND p.current_generation=e.runtime_generation WHERE e.plugin_id=plugin AND e.supported_features ? feature)
$$;
-- Lock alongside enrollment. Numeric capability2 is required; string/bool claims fail closed.
CREATE FUNCTION webdock_rls.lock_plugin_lifecycle_agent(cluster text, generation integer) RETURNS boolean
 LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE a webdock_auth.hosting_agent;
BEGIN
 IF NOT (webdock_rls.operator() OR webdock_rls.worker(ARRAY['plugin-controller']::text[])) THEN RETURN false; END IF;
 SELECT * INTO a FROM webdock_auth.hosting_agent WHERE cluster_id=$1 FOR SHARE;
 RETURN FOUND AND NOT a.revoked AND a.generation=$2
 AND coalesce(a.observation->'capabilities'->'pluginRuntimeVersion'='2'::jsonb,false);
END $$;
ALTER FUNCTION webdock_rls.lock_plugin_lifecycle_agent(text,integer) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.lock_plugin_lifecycle_agent(text,integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.lock_plugin_lifecycle_agent(text,integer) TO webdock_auth_runtime,webdock_worker_runtime;
