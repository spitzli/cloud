-- Native PostgreSQL backup authority. No session is fabricated for recurring CNPG work.
CREATE TABLE webdock_auth.hosting_database_backup_destination (
 id text PRIMARY KEY, spec jsonb NOT NULL, write_encrypted text NOT NULL,
 recovery_encrypted text NOT NULL, created_by text NOT NULL, created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE webdock_auth.hosting_database_backup_policy (
 id text PRIMARY KEY, database_id varchar NOT NULL REFERENCES webdock_auth.hosting_database(id),
 backend_id varchar NOT NULL REFERENCES webdock_auth.hosting_database_backend(id),
 cluster_id varchar NOT NULL REFERENCES webdock_auth.hosting_cluster(id),
 customer_id varchar NOT NULL, project_id varchar NOT NULL,
 destination_id text NOT NULL REFERENCES webdock_auth.hosting_database_backup_destination(id),
 source_uid text NOT NULL, spec jsonb NOT NULL, revision integer NOT NULL CHECK(revision>0),
 approved_by text NOT NULL, approved_at timestamptz NOT NULL DEFAULT now(),
 revoked_at timestamptz, observed_revision integer NOT NULL DEFAULT 0,
 observed_suspended boolean, schedule_uid text, observed_at timestamptz, observed_controls jsonb,
 credentials_encrypted text NOT NULL, credential_revision integer NOT NULL CHECK(credential_revision>0),
 UNIQUE(database_id), UNIQUE(backend_id),
 CHECK(spec->>'id'=id AND spec->>'installationID'=backend_id AND spec->>'customerID'=customer_id
  AND spec->>'projectID'=project_id AND spec->>'sourceCnpgUID'=source_uid
  AND (spec->>'revision')::integer=revision AND spec->'destination'->>'id'=destination_id),
 CHECK(revoked_at IS NULL OR spec->>'suspended'='true')
);
CREATE TABLE webdock_auth.hosting_database_backup_credentials (
 policy_id text NOT NULL REFERENCES webdock_auth.hosting_database_backup_policy(id),
 revision integer NOT NULL CHECK(revision>0), credentials_encrypted text NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now(), PRIMARY KEY(policy_id,revision)
);
CREATE TABLE webdock_auth.hosting_database_backup (
 id text PRIMARY KEY, policy_id text NOT NULL REFERENCES webdock_auth.hosting_database_backup_policy(id),
 policy_revision integer NOT NULL, backup_uid text NOT NULL, base_backup_id text NOT NULL,
 completed_at timestamptz NOT NULL, observed_at timestamptz NOT NULL DEFAULT now(),
 proof jsonb NOT NULL, UNIQUE(policy_id,backup_uid), UNIQUE(policy_id,base_backup_id)
);
ALTER TABLE webdock_auth.hosting_database_job ADD COLUMN backup_policy_id text REFERENCES webdock_auth.hosting_database_backup_policy(id);
ALTER TABLE webdock_auth.hosting_database_job ADD COLUMN backup_policy_revision integer;
ALTER TABLE webdock_auth.hosting_database_job ADD COLUMN recovery jsonb;
-- Audit keeps the human approval/revocation and the independently observed result.
CREATE TABLE webdock_auth.hosting_database_backup_audit (
 id text PRIMARY KEY, policy_id text NOT NULL REFERENCES webdock_auth.hosting_database_backup_policy(id),
 subject text NOT NULL, action text NOT NULL, revision integer NOT NULL, detail jsonb NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE FUNCTION webdock_rls.database_backup_immutable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_TABLE_NAME='hosting_database_backup_policy' THEN
  IF current_user='webdock_worker_runtime' AND (NEW.spec,NEW.revision,NEW.revoked_at) IS DISTINCT FROM (OLD.spec,OLD.revision,OLD.revoked_at) THEN RAISE EXCEPTION 'Agent cannot grant backup authority'; END IF;
  IF current_user='webdock_auth_runtime' AND (NEW.observed_revision,NEW.observed_suspended,NEW.schedule_uid,NEW.observed_at,NEW.observed_controls) IS DISTINCT FROM (OLD.observed_revision,OLD.observed_suspended,OLD.schedule_uid,OLD.observed_at,OLD.observed_controls) THEN RAISE EXCEPTION 'Observed suspension requires agent evidence'; END IF;
  IF (NEW.id,NEW.database_id,NEW.backend_id,NEW.cluster_id,NEW.customer_id,NEW.project_id,NEW.destination_id,NEW.source_uid,NEW.approved_by,NEW.approved_at,NEW.credentials_encrypted,NEW.credential_revision)
   IS DISTINCT FROM (OLD.id,OLD.database_id,OLD.backend_id,OLD.cluster_id,OLD.customer_id,OLD.project_id,OLD.destination_id,OLD.source_uid,OLD.approved_by,OLD.approved_at,OLD.credentials_encrypted,OLD.credential_revision)
   OR NEW.spec->'destination' IS DISTINCT FROM OLD.spec->'destination'
   OR NEW.revision<OLD.revision OR (NEW.spec IS DISTINCT FROM OLD.spec AND NEW.revision<>OLD.revision+1)
   OR (OLD.revoked_at IS NOT NULL AND NEW.revoked_at IS DISTINCT FROM OLD.revoked_at)
   THEN RAISE EXCEPTION 'Immutable backup grant or revision'; END IF;
 ELSE
  RAISE EXCEPTION 'Immutable backup record';
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER backup_policy_immutable BEFORE UPDATE ON webdock_auth.hosting_database_backup_policy FOR EACH ROW EXECUTE FUNCTION webdock_rls.database_backup_immutable();
CREATE TRIGGER backup_destination_immutable BEFORE UPDATE OR DELETE ON webdock_auth.hosting_database_backup_destination FOR EACH ROW EXECUTE FUNCTION webdock_rls.database_backup_immutable();
CREATE TRIGGER backup_credentials_immutable BEFORE UPDATE OR DELETE ON webdock_auth.hosting_database_backup_credentials FOR EACH ROW EXECUTE FUNCTION webdock_rls.database_backup_immutable();
CREATE TRIGGER backup_record_immutable BEFORE UPDATE OR DELETE ON webdock_auth.hosting_database_backup FOR EACH ROW EXECUTE FUNCTION webdock_rls.database_backup_immutable();
CREATE TRIGGER backup_audit_immutable BEFORE UPDATE OR DELETE ON webdock_auth.hosting_database_backup_audit FOR EACH ROW EXECUTE FUNCTION webdock_rls.database_backup_immutable();
DO $$ DECLARE t text; BEGIN
 FOREACH t IN ARRAY ARRAY['hosting_database_backup_destination','hosting_database_backup_policy','hosting_database_backup','hosting_database_backup_audit','hosting_database_backup_credentials'] LOOP
  EXECUTE format('ALTER TABLE webdock_auth.%I OWNER TO webdock_platform_owner',t);
  EXECUTE format('ALTER TABLE webdock_auth.%I ENABLE ROW LEVEL SECURITY',t);
  EXECUTE format('ALTER TABLE webdock_auth.%I FORCE ROW LEVEL SECURITY',t);
  EXECUTE format('CREATE POLICY platform_owner ON webdock_auth.%I TO webdock_platform_owner USING(true) WITH CHECK(true)',t);
  EXECUTE format('GRANT SELECT,INSERT,UPDATE ON webdock_auth.%I TO webdock_auth_runtime,webdock_worker_runtime',t);
 END LOOP;
END $$;
CREATE POLICY operator_destination ON webdock_auth.hosting_database_backup_destination TO webdock_auth_runtime USING(webdock_rls.operator()) WITH CHECK(webdock_rls.operator());
CREATE POLICY agent_destination ON webdock_auth.hosting_database_backup_destination FOR SELECT TO webdock_worker_runtime USING(EXISTS(SELECT 1 FROM webdock_auth.hosting_database_backup_policy p WHERE p.destination_id=hosting_database_backup_destination.id AND webdock_rls.worker(ARRAY['agent'],p.cluster_id)));
CREATE POLICY customer_policy_read ON webdock_auth.hosting_database_backup_policy FOR SELECT TO webdock_auth_runtime USING(webdock_rls.customer_allowed(customer_id));
CREATE POLICY operator_policy_insert ON webdock_auth.hosting_database_backup_policy FOR INSERT TO webdock_auth_runtime WITH CHECK(webdock_rls.operator() AND webdock_rls.project_matches(project_id,customer_id));
CREATE POLICY operator_policy_update ON webdock_auth.hosting_database_backup_policy FOR UPDATE TO webdock_auth_runtime USING(webdock_rls.operator()) WITH CHECK(webdock_rls.operator() AND webdock_rls.project_matches(project_id,customer_id));
CREATE POLICY agent_policy_read ON webdock_auth.hosting_database_backup_policy FOR SELECT TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['agent'],cluster_id));
CREATE POLICY agent_policy_update ON webdock_auth.hosting_database_backup_policy FOR UPDATE TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['agent'],cluster_id)) WITH CHECK(webdock_rls.worker(ARRAY['agent'],cluster_id));
DO $$ DECLARE t text; BEGIN
 FOREACH t IN ARRAY ARRAY['hosting_database_backup','hosting_database_backup_audit','hosting_database_backup_credentials'] LOOP
  EXECUTE format('CREATE POLICY customer_read ON webdock_auth.%I FOR SELECT TO webdock_auth_runtime USING(EXISTS(SELECT 1 FROM webdock_auth.hosting_database_backup_policy p WHERE p.id=policy_id AND webdock_rls.customer_allowed(p.customer_id)))',t);
  EXECUTE format('CREATE POLICY operator_insert ON webdock_auth.%I FOR INSERT TO webdock_auth_runtime WITH CHECK(webdock_rls.operator() AND EXISTS(SELECT 1 FROM webdock_auth.hosting_database_backup_policy p WHERE p.id=policy_id))',t);
  EXECUTE format('CREATE POLICY agent_insert ON webdock_auth.%I FOR INSERT TO webdock_worker_runtime WITH CHECK(EXISTS(SELECT 1 FROM webdock_auth.hosting_database_backup_policy p WHERE p.id=policy_id AND webdock_rls.worker(ARRAY[''agent''],p.cluster_id)))',t);
  EXECUTE format('CREATE POLICY agent_read ON webdock_auth.%I FOR SELECT TO webdock_worker_runtime USING(EXISTS(SELECT 1 FROM webdock_auth.hosting_database_backup_policy p WHERE p.id=policy_id AND webdock_rls.worker(ARRAY[''agent''],p.cluster_id)))',t);
 END LOOP;
END $$;
-- Worker receives only a scoped boolean, never a session or unrestricted customer metadata.
CREATE FUNCTION webdock_rls.database_backup_policy_live(policy text, cluster text) RETURNS boolean
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
 SELECT webdock_rls.worker(ARRAY['agent'],cluster) AND EXISTS(
  SELECT 1 FROM webdock_auth.hosting_database_backup_policy p
  JOIN webdock_auth.hosting_database_backend b ON b.id=p.backend_id
  JOIN webdock_auth.hosting_database d ON d.id=p.database_id
  JOIN webdock_admin.projects pr ON pr.id=p.project_id
  JOIN webdock_admin.customers c ON c.id=p.customer_id
  WHERE p.id=policy AND p.cluster_id=cluster AND b.cluster_id=cluster AND b.cnpg_uid=p.source_uid
   AND b.kind='dedicated' AND b.state<>'purged' AND d.backend_id=p.backend_id
   AND d.customer_id=p.customer_id AND d.project_id=p.project_id AND pr.customer_id=p.customer_id
   AND ((p.revoked_at IS NOT NULL AND p.spec->>'suspended'='true') OR
    (pr.status='active' AND c.status='active' AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion x WHERE x.project_id=pr.id AND x.status<>'deleted'))))
$$;
ALTER FUNCTION webdock_rls.database_backup_policy_live(text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.database_backup_policy_live(text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.database_backup_policy_live(text,text) TO webdock_worker_runtime;
