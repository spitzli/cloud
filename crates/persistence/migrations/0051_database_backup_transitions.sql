-- Independent recurrence/WAL/retention authority must be observed stopped before
-- registry archival. No session or controller principal is created by this fence.
CREATE FUNCTION webdock_rls.database_backup_stopped(p webdock_auth.hosting_database_backup_policy)
RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
 SELECT EXISTS(SELECT 1 FROM webdock_auth.hosting_database_backend b
  WHERE b.id=p.backend_id AND b.cnpg_uid=p.source_uid AND (
   (b.state='purged' AND EXISTS(SELECT 1 FROM webdock_auth.hosting_database_job j
     JOIN webdock_auth.hosting_operation o ON o.id=j.operation_id
     WHERE j.backend_id=b.id AND j.action IN('purge','purgeSource') AND o.status='succeeded'
      AND o.result->>'state'='purged' AND o.result->>'installationID'=b.id AND o.result->>'cnpgUID'=p.source_uid))
   OR (p.spec->>'suspended'='true' AND p.spec->>'walEnabled'='false' AND p.spec->>'retentionDelete'='false'
    AND (p.spec->>'resourceRevision')::integer=b.revision
    AND p.observed_revision=p.revision AND p.observed_suspended AND p.schedule_uid IS NOT NULL
    AND p.observed_at>clock_timestamp()-interval '90 seconds'
    AND p.observed_controls->>'schedule'=p.spec->>'schedule'
    AND p.observed_controls->>'walEnabled'='false' AND p.observed_controls->>'retentionDelete'='false'
    AND length(p.observed_controls->>'scheduledBackupResourceVersion')>0
    AND length(p.observed_controls->>'clusterResourceVersion')>0
    AND length(p.observed_controls->>'objectStoreUID')>0
    AND length(p.observed_controls->>'objectStoreResourceVersion')>0
    AND EXISTS(SELECT 1 FROM webdock_auth.hosting_agent a WHERE a.cluster_id=p.cluster_id AND NOT a.revoked
     AND a.last_seen>clock_timestamp()-interval '90 seconds'
     AND a.observation#>'{capabilities,databaseBackupVersion}'='1'::jsonb))))
$$;
ALTER FUNCTION webdock_rls.database_backup_stopped(webdock_auth.hosting_database_backup_policy) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.database_backup_stopped(webdock_auth.hosting_database_backup_policy) FROM PUBLIC;

CREATE FUNCTION webdock_rls.database_backup_archive_guard() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE p webdock_auth.hosting_database_backup_policy;
BEGIN
 IF NEW.status::text='archived' AND OLD.status::text IS DISTINCT FROM 'archived' THEN
  FOR p IN SELECT policy.* FROM webdock_auth.hosting_database_backup_policy policy
   WHERE (TG_TABLE_NAME='projects' AND policy.project_id=NEW.id)
      OR (TG_TABLE_NAME='customers' AND policy.customer_id=NEW.id)
   ORDER BY policy.id FOR SHARE LOOP
   IF webdock_rls.database_backup_stopped(p) IS NOT TRUE THEN
    RAISE EXCEPTION 'Observe PostgreSQL recurrence, WAL and retention stopped before archival' USING ERRCODE='WD051';
   END IF;
  END LOOP;
 END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_rls.database_backup_archive_guard() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.database_backup_archive_guard() FROM PUBLIC;
CREATE TRIGGER database_backup_project_archive BEFORE UPDATE OF status ON webdock_admin.projects
 FOR EACH ROW EXECUTE FUNCTION webdock_rls.database_backup_archive_guard();
CREATE TRIGGER database_backup_customer_archive BEFORE UPDATE OF status ON webdock_admin.customers
 FOR EACH ROW EXECUTE FUNCTION webdock_rls.database_backup_archive_guard();

-- Serialize scope eligibility with Registry's actual customer/project row locks.
-- Observation-only updates retain their independent, enrolled-agent authority.
CREATE FUNCTION webdock_rls.database_backup_scope_guard() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE customer_state text; project_state text;
BEGIN
 IF TG_OP='UPDATE' AND (NEW.spec,NEW.revision,NEW.revoked_at) IS NOT DISTINCT FROM (OLD.spec,OLD.revision,OLD.revoked_at) THEN RETURN NEW; END IF;
 SELECT status::text INTO customer_state FROM webdock_admin.customers WHERE id=NEW.customer_id FOR SHARE;
 SELECT status::text INTO project_state FROM webdock_admin.projects WHERE id=NEW.project_id AND customer_id=NEW.customer_id FOR SHARE;
 IF customer_state IS DISTINCT FROM 'active' OR project_state IS DISTINCT FROM 'active' THEN
  RAISE EXCEPTION 'Backup authority requires active scope' USING ERRCODE='WD051';
 END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_rls.database_backup_scope_guard() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.database_backup_scope_guard() FROM PUBLIC;
CREATE TRIGGER database_backup_scope BEFORE INSERT OR UPDATE ON webdock_auth.hosting_database_backup_policy
 FOR EACH ROW EXECUTE FUNCTION webdock_rls.database_backup_scope_guard();

-- The ordering worker gets no general policy UPDATE privilege. This function can
-- only advance resource/policy revisions for the exact admitted dedicated resize.
CREATE FUNCTION webdock_rls.resize_database_backup_policy(backend text, previous_revision integer, next_revision integer)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE b webdock_auth.hosting_database_backend; p webdock_auth.hosting_database_backup_policy; scope jsonb;
BEGIN
 SELECT * INTO b FROM webdock_auth.hosting_database_backend WHERE id=backend FOR UPDATE;
 scope:=webdock_rls.request_scope();
 IF NOT FOUND OR NOT webdock_rls.worker(ARRAY['database-order'],b.cluster_id)
  OR NOT webdock_rls.customer_allowed(scope->>'customerID') OR NOT webdock_rls.backend_owned(b.id,true)
  OR b.kind<>'dedicated' OR b.state<>'updating' OR b.revision<>next_revision OR next_revision<>previous_revision+1
  OR NOT EXISTS(SELECT 1 FROM webdock_auth."user" u JOIN webdock_auth.session s ON s."userId"=u.id
    WHERE u.id=scope->>'subject' AND s.id=scope->>'sessionID' AND s."expiresAt">statement_timestamp()
     AND u."emailVerified" AND NOT u."mustChangePassword" AND NOT coalesce(u.banned,false)
     AND (u.role='user' OR (u.role='operator' AND u."twoFactorEnabled"))) THEN
  RAISE EXCEPTION 'Current admitted database resize required' USING ERRCODE='42501';
 END IF;
 SELECT * INTO p FROM webdock_auth.hosting_database_backup_policy WHERE backend_id=backend FOR UPDATE;
 IF NOT FOUND THEN RETURN jsonb_build_object('hasPolicy',false); END IF;
 IF p.customer_id IS DISTINCT FROM scope->>'customerID' OR p.source_uid IS DISTINCT FROM b.cnpg_uid
  OR (p.spec->>'resourceRevision')::integer IS DISTINCT FROM previous_revision
  OR NOT EXISTS(SELECT 1 FROM webdock_auth.hosting_agent a WHERE a.cluster_id=b.cluster_id AND NOT a.revoked
    AND a.last_seen>clock_timestamp()-interval '90 seconds' AND a.observation#>'{capabilities,databaseBackupVersion}'='1'::jsonb)
  OR EXISTS(SELECT 1 FROM webdock_auth.hosting_database_job j JOIN webdock_auth.hosting_operation o ON o.id=j.operation_id
    WHERE j.backend_id=b.id AND j.backup_policy_id IS NOT NULL AND (
      o.status IN('running','needs-reconciliation') OR (o.status='queued' AND (o.generation<>0 OR j.checkpoint IS NOT NULL
       OR j.lease_token_hash IS NOT NULL OR coalesce(jsonb_array_length(o.desired->'nativeIssuedSpecs'),0)>0)))) THEN
  RAISE EXCEPTION 'Finish backup work and qualify current resource authority before resize' USING ERRCODE='WD052';
 END IF;
 UPDATE webdock_auth.hosting_operation o SET status='failed',lease_until=NULL,
  result=jsonb_build_object('error','superseded-by-resize','externalEffects','not-issued')
 FROM webdock_auth.hosting_database_job j WHERE j.operation_id=o.id AND j.backup_policy_id=p.id
  AND o.status='queued' AND o.generation=0 AND j.checkpoint IS NULL AND j.lease_token_hash IS NULL;
 UPDATE webdock_auth.hosting_database_backup_policy SET revision=revision+1,
  spec=jsonb_set(jsonb_set(spec,'{resourceRevision}',to_jsonb(next_revision)),'{revision}',to_jsonb(revision+1))
 WHERE id=p.id;
 RETURN jsonb_build_object('hasPolicy',true,'policyID',p.id,'revision',p.revision+1);
END $$;
ALTER FUNCTION webdock_rls.resize_database_backup_policy(text,integer,integer) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.resize_database_backup_policy(text,integer,integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.resize_database_backup_policy(text,integer,integer) TO webdock_worker_runtime;
