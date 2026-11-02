-- Initial installation authority; availability operations and retained enrollments are unchanged.
CREATE INDEX plugin_operation_history ON webdock_auth.plugin_operation(plugin_id,created_at DESC,id DESC);
CREATE TABLE webdock_auth.plugin_release_verification (
 plugin_id text NOT NULL, version text NOT NULL,
 jws text NOT NULL CHECK(octet_length(jws)<=65536),
 signer_sha256 text NOT NULL CHECK(signer_sha256 ~ '^[a-f0-9]{64}$'),
 format_version integer NOT NULL CHECK(format_version=1),
 PRIMARY KEY(plugin_id,version),
 FOREIGN KEY(plugin_id,version) REFERENCES webdock_auth.plugin_release(plugin_id,version)
);
CREATE TABLE webdock_auth.plugin_install_execution (
 plugin_id text PRIMARY KEY REFERENCES webdock_auth.plugin_installation(id),
 runtime_generation bigint NOT NULL CHECK(runtime_generation>0),
 configuration_revision bigint NOT NULL CHECK(configuration_revision>0),
 operation_id text NOT NULL REFERENCES webdock_auth.plugin_operation(id),
 platform_id text NOT NULL CHECK(platform_id ~ '^[a-z][a-z0-9-]{0,23}$'),
 target_id text NOT NULL, cluster_id varchar NOT NULL REFERENCES webdock_auth.hosting_cluster(id),
 review_sha256 text NOT NULL CHECK(review_sha256 ~ '^[a-f0-9]{64}$'),
 supported_features jsonb NOT NULL CHECK(jsonb_typeof(supported_features)='array'),
 private_components jsonb NOT NULL CHECK(jsonb_typeof(private_components)='object'),
 spec jsonb, spec_sha256 text CHECK(spec_sha256 ~ '^[a-f0-9]{64}$'),
 binding jsonb, checkpoint jsonb,
 agent_generation integer,
 agent_token_sha256 text CHECK(agent_token_sha256 ~ '^[a-f0-9]{64}$'),
 created_at timestamptz NOT NULL DEFAULT now(), updated_at timestamptz NOT NULL DEFAULT now(),
 FOREIGN KEY(plugin_id,runtime_generation) REFERENCES webdock_auth.plugin_runtime(plugin_id,generation),
 CHECK((spec IS NULL AND spec_sha256 IS NULL AND binding IS NULL) OR
       (jsonb_typeof(spec)='object' AND spec_sha256 IS NOT NULL AND jsonb_typeof(binding)='object'))
);
CREATE TABLE webdock_auth.plugin_install_dispatch (
 operation_id text NOT NULL REFERENCES webdock_auth.plugin_operation(id),
 generation bigint NOT NULL CHECK(generation>0),
 token_sha256 text NOT NULL CHECK(token_sha256 ~ '^[a-f0-9]{64}$'),
 spec_sha256 text NOT NULL CHECK(spec_sha256 ~ '^[a-f0-9]{64}$'),
 lease_sha256 text NOT NULL CHECK(lease_sha256 ~ '^[a-f0-9]{64}$'),
 agent_generation integer NOT NULL CHECK(agent_generation>0),
 cluster_id text NOT NULL, platform_id text NOT NULL,
 PRIMARY KEY(operation_id,generation,token_sha256)
);
CREATE TABLE webdock_auth.plugin_install_receipt (
 operation_id text NOT NULL REFERENCES webdock_auth.plugin_operation(id),
 generation bigint NOT NULL CHECK(generation>0),
 token_sha256 text NOT NULL CHECK(token_sha256 ~ '^[a-f0-9]{64}$'),
 spec_sha256 text NOT NULL CHECK(spec_sha256 ~ '^[a-f0-9]{64}$'),
 lease_sha256 text NOT NULL CHECK(lease_sha256 ~ '^[a-f0-9]{64}$'),
 agent_generation integer NOT NULL CHECK(agent_generation>0),
 kind text NOT NULL CHECK(kind IN ('checkpoint','complete')),
 body_sha256 text NOT NULL CHECK(body_sha256 ~ '^[a-f0-9]{64}$'),
 disposition text NOT NULL CHECK(disposition IN ('accepted','obsolete')),
 created_at timestamptz NOT NULL DEFAULT now(),
 PRIMARY KEY(operation_id,generation,token_sha256,kind,body_sha256)
);
DO $$ DECLARE t text; BEGIN
 FOREACH t IN ARRAY ARRAY['plugin_release_verification','plugin_install_execution','plugin_install_dispatch','plugin_install_receipt'] LOOP
  EXECUTE format('ALTER TABLE webdock_auth.%I OWNER TO webdock_platform_owner',t);
  EXECUTE format('ALTER TABLE webdock_auth.%I ENABLE ROW LEVEL SECURITY',t);
  EXECUTE format('ALTER TABLE webdock_auth.%I FORCE ROW LEVEL SECURITY',t);
  EXECUTE format('REVOKE ALL ON webdock_auth.%I FROM PUBLIC',t);
  EXECUTE format('CREATE POLICY installation_owner ON webdock_auth.%I TO webdock_platform_owner USING(true) WITH CHECK(true)',t);
  EXECUTE format('GRANT SELECT,INSERT ON webdock_auth.%I TO webdock_auth_runtime',t);
  EXECUTE format('CREATE POLICY installation_operator_read ON webdock_auth.%I FOR SELECT TO webdock_auth_runtime USING(webdock_rls.operator())',t);
  EXECUTE format('CREATE POLICY installation_operator_insert ON webdock_auth.%I FOR INSERT TO webdock_auth_runtime WITH CHECK(webdock_rls.operator())',t);
  EXECUTE format('GRANT SELECT ON webdock_auth.%I TO webdock_worker_runtime',t);
  EXECUTE format('CREATE POLICY installation_controller_read ON webdock_auth.%I FOR SELECT TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY[''plugin-controller'']::text[]))',t);
 END LOOP;
END $$;
GRANT UPDATE ON webdock_auth.plugin_install_execution TO webdock_auth_runtime,webdock_worker_runtime;
CREATE POLICY installation_operator_update ON webdock_auth.plugin_install_execution FOR UPDATE TO webdock_auth_runtime USING(webdock_rls.operator()) WITH CHECK(webdock_rls.operator());
CREATE POLICY installation_controller_update ON webdock_auth.plugin_install_execution FOR UPDATE TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['plugin-controller']::text[])) WITH CHECK(webdock_rls.worker(ARRAY['plugin-controller']::text[]));
GRANT INSERT ON webdock_auth.plugin_install_receipt TO webdock_worker_runtime;
CREATE POLICY installation_controller_insert ON webdock_auth.plugin_install_receipt FOR INSERT TO webdock_worker_runtime WITH CHECK(webdock_rls.worker(ARRAY['plugin-controller']::text[]));
REVOKE INSERT ON webdock_auth.plugin_install_dispatch,webdock_auth.plugin_install_receipt FROM webdock_auth_runtime;
GRANT INSERT ON webdock_auth.plugin_install_dispatch TO webdock_worker_runtime;
CREATE POLICY installation_controller_insert ON webdock_auth.plugin_install_dispatch FOR INSERT TO webdock_worker_runtime WITH CHECK(webdock_rls.worker(ARRAY['plugin-controller']::text[]));

-- The generic ceiling is independent of a customer's feature mode and applies to operators too.
CREATE FUNCTION webdock_rls.plugin_feature_supported(plugin text, feature text) RETURNS boolean
 LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
 SELECT NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_install_execution e WHERE e.plugin_id=plugin)
 OR EXISTS(SELECT 1 FROM webdock_auth.plugin_install_execution e WHERE e.plugin_id=plugin AND e.supported_features ? feature)
$$;
ALTER FUNCTION webdock_rls.plugin_feature_supported(text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.plugin_feature_supported(text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.plugin_feature_supported(text,text) TO webdock_auth_runtime,webdock_worker_runtime;

CREATE FUNCTION webdock_rls.lock_plugin_install_agent(cluster text, generation integer, platform text) RETURNS boolean
 LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE a webdock_auth.hosting_agent; e webdock_auth.agent_transport_enrollment; c webdock_auth.hosting_cluster;
BEGIN
 IF NOT webdock_rls.worker(ARRAY['plugin-controller']::text[]) THEN RETURN false; END IF;
 SELECT * INTO c FROM webdock_auth.hosting_cluster WHERE id=$1 FOR SHARE;
 IF NOT FOUND OR NOT c.verified OR lower(c.region)<>'eu' OR c.ownership<>'platform' OR c.runtime_installation<>$3 THEN RETURN false; END IF;
 SELECT * INTO a FROM webdock_auth.hosting_agent WHERE cluster_id=$1 FOR SHARE;
 IF NOT FOUND OR a.revoked OR a.generation<>$2 THEN RETURN false; END IF;
 SELECT * INTO e FROM webdock_auth.agent_transport_enrollment WHERE cluster_id=$1 AND installation=$3 AND NOT revoked AND agent_transport_enrollment.generation=$2 AND credential_hash=a.credential_hash FOR SHARE;
 RETURN FOUND;
END $$;
ALTER FUNCTION webdock_rls.lock_plugin_install_agent(text,integer,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.lock_plugin_install_agent(text,integer,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.lock_plugin_install_agent(text,integer,text) TO webdock_worker_runtime;
