-- PROPOSED 0056 ONLY. Not a migration, not executed, no production registration.
-- Current 0054 input SHA256: 411827bcffb9b295e00cda6efee7680aa32d601cab99952be838456a45ba1f5c (draft; rebase/review after its final freeze).
-- Exactly three worker-only interfaces. No tables, columns, policies or general grants.
-- Caller partial order: optional retained agent SHARE -> this ONE early epoch SHARE
-- NOWAIT on the SAME worker transaction -> business native-session-context fence ->
-- retained actual Browser/MCP authentication -> mapped/customer/project -> worker
-- cluster/offer/operation reread/CAS. A miss aborts/releases the whole worker TX
-- before retaining authentication/business locks. Never upgrade agent SHARE or lock
-- Main recovery job/policy/operation rows while holding this epoch. No second epoch
-- acquisition on a business connection. Real overlap/rollback tests remain required.
-- Fence proof is a definer-owned ON COMMIT DROP private temp witness. Acquiring
-- epoch SHARE and inserting the witness share the same subtransaction lifetime:
-- savepoint rollback removes both. Caller-set GUC/top-XID MAC is never proof.
-- Authenticated actor scope and retained actual guards are separately mandatory.

CREATE FUNCTION webdock_rls.database_restore_epoch(expected_epoch uuid DEFAULT NULL)
RETURNS uuid LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE e webdock_auth.native_recovery_epoch%ROWTYPE;
BEGIN
 IF session_user<>'webdock_worker_runtime' THEN
  RAISE EXCEPTION 'Database worker required' USING ERRCODE='42501'; END IF;
 IF has_database_privilege(current_user,current_database(),'TEMP') IS NOT TRUE THEN
  RAISE EXCEPTION 'Existing platform-owner TEMP privilege required' USING ERRCODE='55000'; END IF;
 IF EXISTS(SELECT 1 FROM pg_catalog.pg_class c WHERE c.relnamespace=pg_my_temp_schema() AND c.relname='webdock_restore_epoch_witness') THEN
  RAISE EXCEPTION 'One fresh epoch acquisition; caller-created witness forbidden' USING ERRCODE='42501'; END IF;
 -- No retained installation/customer/auth row lock precedes the early NOWAIT.
 SELECT * INTO e FROM webdock_auth.native_recovery_epoch WHERE singleton FOR SHARE NOWAIT;
 IF NOT FOUND OR e.state IS DISTINCT FROM 'active' OR expected_epoch IS NOT NULL AND e.epoch<>expected_epoch THEN
  RAISE EXCEPTION 'Active matching database epoch required' USING ERRCODE='55000'; END IF;
 CREATE TEMP TABLE pg_temp.webdock_restore_epoch_witness(backend_pid integer NOT NULL,top_xid xid8 NOT NULL,epoch uuid NOT NULL) ON COMMIT DROP;
 REVOKE ALL ON TABLE pg_temp.webdock_restore_epoch_witness FROM PUBLIC,webdock_worker_runtime;
 INSERT INTO pg_temp.webdock_restore_epoch_witness VALUES(pg_backend_pid(),pg_current_xact_id(),e.epoch);
 RETURN e.epoch;
END $$;
ALTER FUNCTION webdock_rls.database_restore_epoch(uuid) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.database_restore_epoch(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.database_restore_epoch(uuid) TO webdock_worker_runtime;

CREATE FUNCTION webdock_rls.database_restore_source(offer_id text,expected_intent_sha256 text)
RETURNS jsonb LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE scope jsonb; offer webdock_auth.hosting_database_offer%ROWTYPE; intent jsonb;
 d record; b record; p record; backup record; destination jsonb;
 v record; safe_policy jsonb; result jsonb; witness_oid oid; witness_count bigint;
BEGIN
 scope:=webdock_rls.request_scope();
 IF session_user<>'webdock_worker_runtime' OR coalesce(expected_intent_sha256,'') !~ '^[a-f0-9]{64}$'
  OR coalesce(webdock_rls.worker(ARRAY['database-order'],scope->>'clusterID'),false) IS NOT TRUE
  OR scope->>'restoreOfferID' IS DISTINCT FROM offer_id
  OR scope->>'restoreIntentHash' IS DISTINCT FROM expected_intent_sha256
  OR scope->>'operator' IS DISTINCT FROM 'true' OR scope->>'canWrite' IS DISTINCT FROM 'true'
  OR scope->>'permission' IS DISTINCT FROM 'org:databases:manage'
  OR jsonb_typeof(scope->'nativeAuthority') IS DISTINCT FROM 'object'
  OR coalesce(scope->'nativeAuthority'->>'kind','') NOT IN('local-session','customer-session','first-party-browser','database-mcp')
  OR NOT EXISTS(SELECT 1 FROM webdock_auth.native_recovery_epoch e WHERE e.singleton AND e.state='active' AND e.epoch::text=scope->>'nativeEpoch') THEN
  RAISE EXCEPTION 'Exact admitted database restore scope required' USING ERRCODE='42501'; END IF;
 -- BEGIN owner-only witness verification (no verify-path creation or epoch lock).
 SELECT c.oid INTO witness_oid FROM pg_catalog.pg_class c
 JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
 WHERE n.oid=pg_my_temp_schema() AND n.nspname~'^pg_temp_[0-9]+$'
  AND c.relname='webdock_restore_epoch_witness' AND c.relkind='r' AND c.relpersistence='t'
  AND c.relowner=(SELECT oid FROM pg_catalog.pg_roles WHERE rolname='webdock_platform_owner')
  AND c.relnatts=3 AND NOT c.relispartition AND NOT c.relhassubclass AND NOT c.relrowsecurity
  AND NOT EXISTS(SELECT 1 FROM pg_catalog.aclexplode(coalesce(c.relacl,pg_catalog.acldefault('r',c.relowner))) a WHERE a.grantee<>c.relowner)
  AND (SELECT jsonb_agg(jsonb_build_array(a.attname,a.atttypid::integer,a.attnotnull) ORDER BY a.attnum)
       FROM pg_catalog.pg_attribute a WHERE a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped)
   =jsonb_build_array(jsonb_build_array('backend_pid','int4'::regtype::integer,true),jsonb_build_array('top_xid','xid8'::regtype::integer,true),jsonb_build_array('epoch','uuid'::regtype::integer,true))
  AND NOT EXISTS(SELECT 1 FROM pg_catalog.pg_trigger t WHERE t.tgrelid=c.oid AND NOT t.tgisinternal);
 IF witness_oid IS NULL THEN
  RAISE EXCEPTION 'Actual early database epoch witness required' USING ERRCODE='42501'; END IF;
 SELECT count(*) INTO witness_count FROM pg_temp.webdock_restore_epoch_witness
  WHERE backend_pid=pg_backend_pid() AND top_xid=pg_current_xact_id() AND epoch::text=scope->>'nativeEpoch';
 IF witness_count<>1 OR (SELECT count(*) FROM pg_temp.webdock_restore_epoch_witness)<>1 THEN
  RAISE EXCEPTION 'Actual early database epoch witness required' USING ERRCODE='42501'; END IF;
 -- END owner-only witness verification.
 -- The caller already holds native session/access and mapped/business guards.
 -- These checks independently reject stale signed context; they mint no session.
 IF NOT EXISTS(SELECT 1 FROM webdock_auth."user" u JOIN webdock_auth.session s ON s."userId"=u.id
   JOIN webdock_admin.users mapped ON mapped.auth_subject=u.id AND mapped.role='operator'
   WHERE u.id=scope->>'subject' AND s.id=scope->>'sessionID' AND s."expiresAt">clock_timestamp()
    AND s."impersonatedBy" IS NULL AND u.role='operator' AND u."twoFactorEnabled" AND u."emailVerified"
    AND NOT coalesce(u.banned,false) AND NOT u."mustChangePassword"
    AND webdock_rls.native_identity_authority(u.id)) THEN
  RAISE EXCEPTION 'Current native operator required' USING ERRCODE='42501'; END IF;
 IF NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_installation WHERE id='databases' AND state='enabled') THEN
  RAISE EXCEPTION 'Databases installation unavailable' USING ERRCODE='55000'; END IF;
 SELECT * INTO offer FROM webdock_auth.hosting_database_offer o WHERE o.id=offer_id FOR SHARE;
 IF NOT FOUND OR offer.customer_id IS DISTINCT FROM scope->>'customerID' OR offer.project_id IS DISTINCT FROM scope->>'projectID'
  OR offer.expires_at<=clock_timestamp() OR offer.snapshot->>'purpose' IS DISTINCT FROM 'restore'
  OR offer.snapshot->>'intentHash' IS DISTINCT FROM expected_intent_sha256
  OR jsonb_typeof(offer.snapshot->'restoreIntent') IS DISTINCT FROM 'object'
  OR octet_length(offer.snapshot::text)>65536 THEN
  RAISE EXCEPTION 'Current immutable restore offer required' USING ERRCODE='42501'; END IF;
 -- This compares the canonical RUST hash stored with the immutable offer. Never
 -- hash jsonb::text: its representation is not the canonical Rust wire encoding.
 intent:=offer.snapshot->'restoreIntent';
 IF NOT (intent ?& ARRAY['version','purpose','offerID','expiresAt','source','target','targetTime','timeline','verificationSHA256'])
  OR jsonb_typeof(intent->'source') IS DISTINCT FROM 'object' OR jsonb_typeof(intent->'target') IS DISTINCT FROM 'object'
  OR NOT ((intent->'source') ?& ARRAY['databaseID','revision','clusterID','policy','restorePointID','baseBackupID','completedAt','credentialRevision'])
  OR NOT ((intent->'target') ?& ARRAY['customerID','projectID','clusterID','planRevisionID','name','plan']) THEN
  RAISE EXCEPTION 'Complete immutable intent required' USING ERRCODE='42501'; END IF;
 IF intent->>'purpose' IS DISTINCT FROM 'restore' OR intent->>'version' IS DISTINCT FROM '1'
  OR intent->>'offerID' IS DISTINCT FROM offer.id OR (intent->>'expiresAt')::timestamptz IS DISTINCT FROM offer.expires_at
  OR intent#>>'{target,customerID}' IS DISTINCT FROM offer.customer_id OR intent#>>'{target,projectID}' IS DISTINCT FROM offer.project_id
  OR intent#>>'{target,planRevisionID}' IS DISTINCT FROM offer.plan_revision_id OR intent#>>'{target,name}' IS DISTINCT FROM offer.name
  OR intent#>>'{target,clusterID}' IS DISTINCT FROM scope->>'clusterID'
  OR intent#>>'{source,clusterID}' IS DISTINCT FROM scope->>'clusterID'
  OR jsonb_typeof(intent->'targetTime') IS DISTINCT FROM 'string'
  OR coalesce(intent->>'verificationSHA256','') !~ '^[a-f0-9]{64}$' OR coalesce((intent->>'timeline')::bigint,0) NOT BETWEEN 1 AND 4294967295
  OR intent#>>'{target,plan,kind}' IS DISTINCT FROM 'dedicated'
  OR offer.snapshot->>'billingStarts' IS DISTINCT FROM 'observed-ready'
  OR offer.snapshot->>'backupStatus' IS DISTINCT FROM 'not-configured'
  OR offer.snapshot->>'billing' IS DISTINCT FROM (CASE WHEN (intent#>'{target,plan}') ? 'sponsoredCustomerID' THEN 'sponsored' ELSE 'monthly-net-fixed' END)
  OR (offer.snapshot - ARRAY['purpose','restoreIntent','intentHash','backupStatus','billing','billingStarts']) IS DISTINCT FROM intent#>'{target,plan}' THEN
  RAISE EXCEPTION 'Exact immutable restore bindings required' USING ERRCODE='42501'; END IF;
 IF NOT EXISTS(SELECT 1 FROM webdock_auth.hosting_database_plan plan WHERE plan.id=offer.plan_revision_id
   AND plan.plan=intent#>'{target,plan}' AND plan.availability IN('pilot','public')
   AND (NOT (plan.plan ? 'sponsoredCustomerID') OR plan.availability='pilot' AND plan.plan->>'sponsoredCustomerID'=offer.customer_id)) THEN
  RAISE EXCEPTION 'Current matching target plan required' USING ERRCODE='42501'; END IF;
 PERFORM c.id FROM webdock_admin.customers c JOIN webdock_auth.tenant_customer t ON t.customer_id=c.id
  WHERE c.id=offer.customer_id AND c.status='active' AND webdock_rls.native_organization_authority(t.organization_id) FOR SHARE OF c;
 IF NOT FOUND THEN RAISE EXCEPTION 'Active native customer required' USING ERRCODE='42501'; END IF;
 PERFORM pr.id FROM webdock_admin.projects pr WHERE pr.id=offer.project_id AND pr.customer_id=offer.customer_id AND pr.status='active'
  AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion x WHERE x.project_id=pr.id AND x.status IS DISTINCT FROM 'deleted') FOR SHARE OF pr;
 IF NOT FOUND THEN RAISE EXCEPTION 'Active same-project target required' USING ERRCODE='42501'; END IF;
 SELECT id,customer_id,project_id,kind,state,pending_backend_id,pending_order_id,revision,backend_id INTO d FROM webdock_auth.hosting_database WHERE id=intent#>>'{source,databaseID}' FOR SHARE;
 IF NOT FOUND OR d.customer_id IS DISTINCT FROM offer.customer_id OR d.project_id IS DISTINCT FROM offer.project_id OR d.kind IS DISTINCT FROM 'dedicated' OR d.state='purged'
  OR d.pending_backend_id IS NOT NULL OR d.pending_order_id IS NOT NULL OR d.revision IS DISTINCT FROM (intent#>>'{source,revision}')::integer THEN
  RAISE EXCEPTION 'Current dedicated source required' USING ERRCODE='42501'; END IF;
 -- 55P03 propagates. Caller must abort ALL worker/business/auth transactions;
 -- no savepoint retry while retaining cluster/policy/auth or epoch guards.
 SELECT id,kind,state,cluster_id,cnpg_uid,revision INTO b FROM webdock_auth.hosting_database_backend WHERE id=d.backend_id FOR SHARE NOWAIT;
 IF NOT FOUND OR b.kind IS DISTINCT FROM 'dedicated' OR b.state='purged' OR b.cluster_id IS DISTINCT FROM scope->>'clusterID'
  OR b.id IS DISTINCT FROM intent#>>'{source,policy,installationID}' OR b.cnpg_uid IS DISTINCT FROM intent#>>'{source,policy,sourceCnpgUID}'
  OR b.revision IS DISTINCT FROM (intent#>>'{source,policy,resourceRevision}')::integer THEN
  RAISE EXCEPTION 'Exact source UID/resource revision required' USING ERRCODE='42501'; END IF;
 SELECT id,database_id,backend_id,cluster_id,customer_id,project_id,source_uid,revoked_at,revision,spec,destination_id INTO p FROM webdock_auth.hosting_database_backup_policy WHERE id=intent#>>'{source,policy,id}' FOR SHARE;
 IF NOT FOUND OR p.database_id IS DISTINCT FROM d.id OR p.backend_id IS DISTINCT FROM b.id OR p.cluster_id IS DISTINCT FROM b.cluster_id OR p.customer_id IS DISTINCT FROM d.customer_id OR p.project_id IS DISTINCT FROM d.project_id
  OR p.source_uid IS DISTINCT FROM b.cnpg_uid OR p.revoked_at IS NOT NULL OR p.revision IS DISTINCT FROM (intent#>>'{source,policy,revision}')::integer
  OR p.spec IS DISTINCT FROM intent#>'{source,policy}' THEN
  RAISE EXCEPTION 'Current bound archive policy required' USING ERRCODE='42501'; END IF;
 SELECT spec INTO destination FROM webdock_auth.hosting_database_backup_destination WHERE id=p.destination_id FOR SHARE;
 IF NOT FOUND OR destination->>'id' IS DISTINCT FROM p.destination_id OR p.spec#>>'{destination,id}' IS DISTINCT FROM p.destination_id
  OR ((p.spec->'destination') - ARRAY['archivePath','serverName']) IS DISTINCT FROM (destination - ARRAY['archivePath','serverName'])
  OR p.spec#>>'{destination,archivePath}' IS DISTINCT FROM 'postgresql/'||p.customer_id||'/'||p.project_id||'/'||p.backend_id||'/'||p.id
  OR p.spec#>>'{destination,serverName}' IS DISTINCT FROM 'wd-'||p.backend_id||'-'||p.id THEN
  RAISE EXCEPTION 'Exact destination credential and key identity required' USING ERRCODE='42501'; END IF;
 SELECT id,policy_revision,backup_uid,base_backup_id,completed_at,proof INTO backup FROM webdock_auth.hosting_database_backup WHERE id=intent#>>'{source,restorePointID}' AND policy_id=p.id;
 IF NOT FOUND OR backup.base_backup_id IS DISTINCT FROM intent#>>'{source,baseBackupID}'
  OR backup.completed_at IS DISTINCT FROM (intent#>>'{source,completedAt}')::timestamptz
  OR backup.policy_revision>p.revision OR (backup.proof->>'revision')::integer IS DISTINCT FROM backup.policy_revision OR backup.proof->>'policyID' IS DISTINCT FROM p.id
  OR backup.proof->>'backupUID' IS DISTINCT FROM backup.backup_uid OR backup.proof->>'baseBackupId' IS DISTINCT FROM backup.base_backup_id
  OR regexp_replace(backup.proof->>'completedAt', '(\.[0-9]{6})[0-9]+', '\1')::timestamptz IS DISTINCT FROM backup.completed_at
  OR intent->>'targetTime' ~ '\.[0-9]{7,}' OR (intent->>'targetTime')::timestamptz<backup.completed_at OR (intent->>'targetTime')::timestamptz>clock_timestamp() THEN
  RAISE EXCEPTION 'Exact completed base and bounded target time required' USING ERRCODE='42501'; END IF;
 SELECT revision,credentials_encrypted INTO v FROM webdock_auth.hosting_database_backup_credentials
  WHERE policy_id=p.id AND revision=(intent#>>'{source,credentialRevision}')::integer;
 IF NOT FOUND OR v.revision<=0 OR octet_length(v.credentials_encrypted)>32768 OR v.credentials_encrypted NOT LIKE 'v1.%' THEN
  RAISE EXCEPTION 'Exact retained historical role credentials required' USING ERRCODE='42501'; END IF;
 -- Explicit whitelist at every nested level: never return the destination ROW,
 -- writer/recovery provider envelopes, live source role envelopes or arbitrary proof.
 safe_policy:=jsonb_build_object('id',p.id,'installationID',p.backend_id,'customerID',p.customer_id,'projectID',p.project_id,
  'sourceCnpgUID',p.source_uid,'revision',p.revision,'resourceRevision',p.spec->'resourceRevision',
  'schedule',p.spec->'schedule','suspended',p.spec->'suspended','walEnabled',p.spec->'walEnabled',
  'retentionDays',p.spec->'retentionDays','retentionDelete',p.spec->'retentionDelete',
  'destination',jsonb_build_object('id',p.destination_id,'endpointURL',destination->'endpointURL','region',destination->'region','bucket',destination->'bucket',
    'archivePath',p.spec#>'{destination,archivePath}','serverName',p.spec#>'{destination,serverName}',
    'credentialID',destination->'credentialID','credentialRevision',destination->'credentialRevision',
    'recoveryCredentialID',destination->'recoveryCredentialID','recoveryCredentialRevision',destination->'recoveryCredentialRevision','keyRecoveryID',destination->'keyRecoveryID'),
  'sidecar',jsonb_build_object('cpuMillicores',p.spec#>'{sidecar,cpuMillicores}','memoryBytes',p.spec#>'{sidecar,memoryBytes}','ephemeralBytes',p.spec#>'{sidecar,ephemeralBytes}'),
  'recoveryJob',jsonb_build_object('cpuMillicores',p.spec#>'{recoveryJob,cpuMillicores}','memoryBytes',p.spec#>'{recoveryJob,memoryBytes}','ephemeralBytes',p.spec#>'{recoveryJob,ephemeralBytes}'));
 result:=jsonb_build_object('source',jsonb_build_object('databaseID',d.id,'revision',d.revision,'clusterID',b.cluster_id,'policy',safe_policy,
  'restorePointID',backup.id,'baseBackupID',backup.base_backup_id,'completedAt',backup.completed_at,'credentialRevision',v.revision),
  'credentialsEncrypted',v.credentials_encrypted);
 IF octet_length(result::text)>65536 THEN RAISE EXCEPTION 'Bounded recovery source required' USING ERRCODE='54000'; END IF;
 RETURN result;
END $$;
ALTER FUNCTION webdock_rls.database_restore_source(text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.database_restore_source(text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.database_restore_source(text,text) TO webdock_worker_runtime;

CREATE FUNCTION webdock_rls.record_database_restore_admission(operation_id text)
RETURNS void LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE scope jsonb; op webdock_auth.hosting_operation%ROWTYPE; j webdock_auth.hosting_database_job%ROWTYPE;
 target webdock_auth.hosting_database%ROWTYPE; backend webdock_auth.hosting_database_backend%ROWTYPE;
 accepted webdock_auth.hosting_database_order%ROWTYPE; offer webdock_auth.hosting_database_offer%ROWTYPE;
 material jsonb; intent jsonb; expected_recovery jsonb; witness_oid oid; witness_count bigint;
 allocation jsonb; fact jsonb; actual jsonb; insert_xid xid8; seen text[]:='{}'; expected_id text;
BEGIN
 scope:=webdock_rls.request_scope();
 IF session_user<>'webdock_worker_runtime' OR webdock_rls.worker(ARRAY['database-order'],scope->>'clusterID') IS NOT TRUE THEN
  RAISE EXCEPTION 'Database admission worker required' USING ERRCODE='42501'; END IF;
 -- BEGIN owner-only witness verification (no verify-path creation or epoch lock).
 SELECT c.oid INTO witness_oid FROM pg_catalog.pg_class c
 JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
 WHERE n.oid=pg_my_temp_schema() AND n.nspname~'^pg_temp_[0-9]+$'
  AND c.relname='webdock_restore_epoch_witness' AND c.relkind='r' AND c.relpersistence='t'
  AND c.relowner=(SELECT oid FROM pg_catalog.pg_roles WHERE rolname='webdock_platform_owner')
  AND c.relnatts=3 AND NOT c.relispartition AND NOT c.relhassubclass AND NOT c.relrowsecurity
  AND NOT EXISTS(SELECT 1 FROM pg_catalog.aclexplode(coalesce(c.relacl,pg_catalog.acldefault('r',c.relowner))) a WHERE a.grantee<>c.relowner)
  AND (SELECT jsonb_agg(jsonb_build_array(a.attname,a.atttypid::integer,a.attnotnull) ORDER BY a.attnum)
       FROM pg_catalog.pg_attribute a WHERE a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped)
   =jsonb_build_array(jsonb_build_array('backend_pid','int4'::regtype::integer,true),jsonb_build_array('top_xid','xid8'::regtype::integer,true),jsonb_build_array('epoch','uuid'::regtype::integer,true))
  AND NOT EXISTS(SELECT 1 FROM pg_catalog.pg_trigger t WHERE t.tgrelid=c.oid AND NOT t.tgisinternal);
 IF witness_oid IS NULL THEN
  RAISE EXCEPTION 'Actual early database epoch witness required' USING ERRCODE='42501'; END IF;
 SELECT count(*) INTO witness_count FROM pg_temp.webdock_restore_epoch_witness
  WHERE backend_pid=pg_backend_pid() AND top_xid=pg_current_xact_id() AND epoch::text=scope->>'nativeEpoch';
 IF witness_count<>1 OR (SELECT count(*) FROM pg_temp.webdock_restore_epoch_witness)<>1 THEN
  RAISE EXCEPTION 'Actual early database epoch witness required' USING ERRCODE='42501'; END IF;
 -- END owner-only witness verification.
 SELECT * INTO op FROM webdock_auth.hosting_operation WHERE id=operation_id FOR UPDATE;
 SELECT * INTO j FROM webdock_auth.hosting_database_job WHERE hosting_database_job.operation_id=record_database_restore_admission.operation_id FOR SHARE;
 IF op.id IS NULL OR j.operation_id IS NULL OR op.action IS DISTINCT FROM 'databases.restore' OR j.action IS DISTINCT FROM 'restore'
  OR op.status IS DISTINCT FROM 'queued' OR op.generation IS DISTINCT FROM 0 OR op.lease_until IS NOT NULL OR j.lease_token_hash IS NOT NULL OR j.checkpoint IS NOT NULL
  OR op.result IS NOT NULL OR j.backup_policy_id IS NOT NULL OR j.backup_policy_revision IS NOT NULL OR j.migration_id IS NOT NULL
  OR op.subject IS DISTINCT FROM scope->>'subject' OR op.actor_session IS DISTINCT FROM scope->>'sessionID'
  OR op.customer_id IS DISTINCT FROM scope->>'customerID' OR op.project_id IS DISTINCT FROM scope->>'projectID' OR op.cluster_id IS DISTINCT FROM scope->>'clusterID'
  OR op.desired->>'restoreOfferID' IS DISTINCT FROM scope->>'restoreOfferID' OR op.desired->>'restoreIntentHash' IS DISTINCT FROM scope->>'restoreIntentHash'
  OR op.desired->>'nativeEpoch' IS DISTINCT FROM scope->>'nativeEpoch' OR op.desired->'nativeAuthority' IS DISTINCT FROM scope->'nativeAuthority'
  OR (op.desired ? 'nativeIssuedSpecs' AND op.desired->'nativeIssuedSpecs'<>'[]'::jsonb) THEN
  RAISE EXCEPTION 'Fresh unissued direct Restore required' USING ERRCODE='42501'; END IF;
 SELECT * INTO target FROM webdock_auth.hosting_database WHERE id=j.database_id FOR SHARE;
 SELECT * INTO backend FROM webdock_auth.hosting_database_backend WHERE id=j.backend_id FOR SHARE;
 SELECT * INTO accepted FROM webdock_auth.hosting_database_order WHERE id=target.order_id FOR SHARE;
 SELECT * INTO offer FROM webdock_auth.hosting_database_offer WHERE id=accepted.offer_id FOR SHARE;
 IF target.id IS NULL OR backend.id IS NULL OR accepted.id IS NULL OR offer.id IS NULL
  OR target.backend_id IS DISTINCT FROM backend.id OR target.order_id IS DISTINCT FROM accepted.id OR accepted.database_id IS DISTINCT FROM target.id
  OR target.customer_id IS DISTINCT FROM op.customer_id OR target.project_id IS DISTINCT FROM op.project_id OR backend.cluster_id IS DISTINCT FROM op.cluster_id
  OR target.database_name IS DISTINCT FROM 'db_'||target.id OR backend.max_databases IS DISTINCT FROM 1 OR backend.logical_capacity_bytes IS DISTINCT FROM 0
  OR target.kind IS DISTINCT FROM 'dedicated' OR backend.kind IS DISTINCT FROM 'dedicated' OR target.state IS DISTINCT FROM 'restoring' OR backend.state IS DISTINCT FROM 'provisioning'
  OR backend.cnpg_uid IS NOT NULL OR backend.revision IS DISTINCT FROM 1 OR backend.observed_revision IS DISTINCT FROM 0 OR backend.endpoint_ready IS DISTINCT FROM false
  OR target.revision IS DISTINCT FROM 1 OR target.observed_revision IS DISTINCT FROM 0 OR j.revision IS DISTINCT FROM target.revision OR op.target_revision IS DISTINCT FROM target.revision
  OR target.operation_id IS DISTINCT FROM op.id OR target.confirmed_credential_revision IS DISTINCT FROM 0 OR target.confirmed_credentials_encrypted IS NOT NULL
  OR target.pending_backend_id IS NOT NULL OR target.pending_order_id IS NOT NULL OR target.pending_plan_revision_id IS NOT NULL
  OR accepted.offer_id IS DISTINCT FROM scope->>'restoreOfferID' OR accepted.accepted_by IS DISTINCT FROM op.subject
  OR accepted.customer_id IS DISTINCT FROM target.customer_id OR accepted.project_id IS DISTINCT FROM target.project_id OR accepted.snapshot IS DISTINCT FROM offer.snapshot
  OR accepted.plan_revision_id IS DISTINCT FROM offer.plan_revision_id OR target.plan_revision_id IS DISTINCT FROM offer.plan_revision_id OR target.name IS DISTINCT FROM offer.name
  OR op.desired->>'restoreOrderID' IS DISTINCT FROM accepted.id OR offer.snapshot->>'purpose' IS DISTINCT FROM 'restore' THEN
  RAISE EXCEPTION 'Exact fresh owned order target required' USING ERRCODE='42501'; END IF;
 -- Facts below are authenticated by the existing private request_scope RLS MAC.
 -- ONLY the trusted INSERT...RETURNING constructor mints them; no Deserialize or
 -- public signer accepts supplied IDs. Actual insertion sub-XIDs survive RELEASE
 -- but must remain in progress; ROLLBACK aborts them even if the MAC was copied.
 allocation:=scope->'restoreAllocation';
 IF jsonb_typeof(allocation) IS DISTINCT FROM 'object' OR allocation->>'version' IS DISTINCT FROM '1'
  OR allocation->>'topXid8' IS DISTINCT FROM pg_current_xact_id()::text OR allocation->>'epoch' IS DISTINCT FROM scope->>'nativeEpoch'
  OR allocation->>'offerID' IS DISTINCT FROM offer.id OR allocation->>'intentHash' IS DISTINCT FROM scope->>'restoreIntentHash'
  OR allocation->>'subject' IS DISTINCT FROM op.subject OR allocation->>'sessionID' IS DISTINCT FROM op.actor_session
  OR allocation->>'backendID' IS DISTINCT FROM backend.id OR allocation->>'orderID' IS DISTINCT FROM accepted.id
  OR allocation->>'databaseID' IS DISTINCT FROM target.id OR jsonb_typeof(allocation->'rows') IS DISTINCT FROM 'array' THEN
  RAISE EXCEPTION 'Authenticated fresh INSERT allocation required' USING ERRCODE='42501'; END IF;
 IF jsonb_array_length(allocation->'rows')<>7 THEN RAISE EXCEPTION 'Exact seven allocation tuples required' USING ERRCODE='42501'; END IF;
 FOR fact IN SELECT value FROM jsonb_array_elements(allocation->'rows') LOOP
  IF jsonb_typeof(fact) IS DISTINCT FROM 'object' OR coalesce(fact->>'kind','') NOT IN('backend','order','database','port','operation','job')
   OR coalesce(fact->>'id','')='' OR coalesce(fact->>'insertXid8','') !~ '^[0-9]{1,20}$'
   OR coalesce(fact->>'tableOid','') !~ '^[0-9]{1,10}$' OR coalesce(fact->>'xmin','') !~ '^[0-9]{1,10}$'
   OR coalesce(fact->>'ctid','') !~ '^\([0-9]+,[0-9]+\)$' OR (fact->>'kind')||':'||(fact->>'id')=ANY(seen) THEN
   RAISE EXCEPTION 'Authenticated exact allocation tuple required' USING ERRCODE='42501'; END IF;
  seen:=array_append(seen,(fact->>'kind')||':'||(fact->>'id'));
  insert_xid:=(fact->>'insertXid8')::xid8;
  IF insert_xid<pg_current_xact_id() OR pg_xact_status(insert_xid) IS DISTINCT FROM 'in progress' THEN
   RAISE EXCEPTION 'Insertion subtransaction was not retained' USING ERRCODE='42501'; END IF;
  IF fact->>'kind'='backend' THEN
   expected_id:=backend.id;
   SELECT jsonb_build_object('tableOid',x.tableoid::oid::text,'xmin',x.xmin::text,'ctid',x.ctid::text) INTO actual FROM webdock_auth.hosting_database_backend x WHERE x.id=backend.id;
  ELSIF fact->>'kind'='order' THEN
   expected_id:=accepted.id;
   SELECT jsonb_build_object('tableOid',x.tableoid::oid::text,'xmin',x.xmin::text,'ctid',x.ctid::text) INTO actual FROM webdock_auth.hosting_database_order x WHERE x.id=accepted.id;
  ELSIF fact->>'kind'='database' THEN
   expected_id:=target.id;
   SELECT jsonb_build_object('tableOid',x.tableoid::oid::text,'xmin',x.xmin::text,'ctid',x.ctid::text) INTO actual FROM webdock_auth.hosting_database x WHERE x.id=target.id;
  ELSIF fact->>'kind'='operation' THEN
   expected_id:=op.id;
   SELECT jsonb_build_object('tableOid',x.tableoid::oid::text,'xmin',x.xmin::text,'ctid',x.ctid::text) INTO actual FROM webdock_auth.hosting_operation x WHERE x.id=op.id;
  ELSIF fact->>'kind'='job' THEN
   expected_id:=op.id;
   SELECT jsonb_build_object('tableOid',x.tableoid::oid::text,'xmin',x.xmin::text,'ctid',x.ctid::text) INTO actual FROM webdock_auth.hosting_database_job x WHERE x.operation_id=op.id;
  ELSE
   IF fact->>'id' NOT IN(backend.direct_port::text,backend.pooled_port::text) THEN RAISE EXCEPTION 'Exact allocated port required' USING ERRCODE='42501'; END IF;
   expected_id:=fact->>'id';
   SELECT jsonb_build_object('tableOid',x.tableoid::oid::text,'xmin',x.xmin::text,'ctid',x.ctid::text) INTO actual FROM webdock_auth.hosting_database_port x WHERE x.backend_id=backend.id AND x.cluster_id=backend.cluster_id AND x.port::text=expected_id;
  END IF;
  IF fact->>'id' IS DISTINCT FROM expected_id OR actual IS NULL OR actual IS DISTINCT FROM (fact - ARRAY['id','kind','insertXid8']) THEN
   RAISE EXCEPTION 'Allocation tuple was replaced or changed after trusted capture' USING ERRCODE='42501'; END IF;
 END LOOP;
 IF NOT (seen @> ARRAY['backend:'||backend.id,'order:'||accepted.id,'database:'||target.id,'port:'||backend.direct_port::text,'port:'||backend.pooled_port::text,'operation:'||op.id,'job:'||op.id])
  OR (SELECT count(*) FROM webdock_auth.hosting_database_port x WHERE x.backend_id=backend.id) IS DISTINCT FROM 2::bigint
  OR EXISTS(SELECT 1 FROM webdock_auth.hosting_database x WHERE (x.backend_id=backend.id OR x.pending_backend_id=backend.id) AND x.id IS DISTINCT FROM target.id)
  OR EXISTS(SELECT 1 FROM webdock_auth.hosting_database_job x WHERE x.backend_id=backend.id AND x.operation_id IS DISTINCT FROM op.id)
  OR EXISTS(SELECT 1 FROM webdock_auth.hosting_database_migration x WHERE x.source_backend_id=backend.id OR x.target_backend_id=backend.id) THEN
  RAISE EXCEPTION 'Fresh backend cannot serve another database, job or migration' USING ERRCODE='42501'; END IF;
 -- Reuses already held PG source row SHAREs; does not acquire the epoch again or
 -- lock any Main recovery job/policy/operation. Only exact safe ROLE ciphertext.
 material:=webdock_rls.database_restore_source(offer.id,scope->>'restoreIntentHash');
 intent:=offer.snapshot->'restoreIntent';
 expected_recovery:=jsonb_build_object('source',material#>'{source,policy}','sourceDatabaseID',material#>'{source,databaseID}',
  'baseBackupId',material#>'{source,baseBackupID}','targetTime',intent->'targetTime','timeline',intent->'timeline',
  'credentialRevision',material#>'{source,credentialRevision}','verificationSHA256',intent->'verificationSHA256');
 IF j.source_backend_id IS DISTINCT FROM material#>>'{source,policy,installationID}' OR backend.id=j.source_backend_id OR target.id=material#>>'{source,databaseID}'
  OR op.desired->>'restorePriceAccepted' IS DISTINCT FROM 'true'
  OR op.desired->'restorePrice' IS DISTINCT FROM jsonb_build_object('monthlyPriceCents',intent#>'{target,plan,monthlyPriceCents}','currency',intent#>'{target,plan,currency}','billing',offer.snapshot->'billing','billingStarts','observed-ready')
  OR j.recovery IS DISTINCT FROM expected_recovery OR target.credential_revision IS DISTINCT FROM (material#>>'{source,credentialRevision}')::integer
  OR backend.profile IS DISTINCT FROM jsonb_build_object('cpuMillicores',intent#>'{target,plan,cpuMillicores}','memoryBytes',intent#>'{target,plan,memoryBytes}','volumeBytes',intent#>'{target,plan,volumeBytes}')
  OR backend.demand->'apps' IS DISTINCT FROM '2'::jsonb OR backend.demand->'replicasPerApp' IS DISTINCT FROM '1'::jsonb
  OR backend.demand->'concurrentDeployments' IS DISTINCT FROM '1'::jsonb OR backend.demand->'volumeBytes' IS DISTINCT FROM backend.profile->'volumeBytes'
  OR (backend.demand->>'cpuMillicores')::bigint IS DISTINCT FROM (backend.profile->>'cpuMillicores')::bigint+200+(material#>>'{source,policy,sidecar,cpuMillicores}')::bigint+(material#>>'{source,policy,recoveryJob,cpuMillicores}')::bigint
  OR (backend.demand->>'memoryBytes')::bigint IS DISTINCT FROM (backend.profile->>'memoryBytes')::bigint+536870912+(material#>>'{source,policy,sidecar,memoryBytes}')::bigint+(material#>>'{source,policy,recoveryJob,memoryBytes}')::bigint
  OR (backend.demand->>'ephemeralBytes')::bigint IS DISTINCT FROM 536870912+(material#>>'{source,policy,sidecar,ephemeralBytes}')::bigint+(material#>>'{source,policy,recoveryJob,ephemeralBytes}')::bigint THEN
  RAISE EXCEPTION 'Exact recovery packet, historical role revision and peak reservation required' USING ERRCODE='42501'; END IF;
 INSERT INTO webdock_auth.hosting_database_backup_audit(id,policy_id,subject,action,revision,detail)
 VALUES('restore-admission:'||op.id,material#>>'{source,policy,id}',op.subject,'backup.restore',(material#>>'{source,policy,revision}')::integer,
  jsonb_build_object('state','queued','operationID',op.id,'orderID',accepted.id,'databaseID',target.id,'installationID',backend.id,
   'sourceDatabaseID',material#>>'{source,databaseID}','restorePointID',material#>>'{source,restorePointID}',
   'intentHash',scope->>'restoreIntentHash','epoch',scope->>'nativeEpoch'));
 -- Admission is not an external-effect receipt or Ready/SQL recovery assertion.
END $$;
ALTER FUNCTION webdock_rls.record_database_restore_admission(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.record_database_restore_admission(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.record_database_restore_admission(text) TO webdock_worker_runtime;
