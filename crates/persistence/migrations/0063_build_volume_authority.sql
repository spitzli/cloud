-- Opt-in fixed volume authority; existing clusters/attempts remain trusted-pilot mode.
ALTER TABLE webdock_auth.git_build_cluster
 ADD COLUMN storage_policy jsonb,
 ADD COLUMN storage_runtime_fingerprint text,
 ADD CONSTRAINT git_build_cluster_storage_policy CHECK (
  (storage_policy IS NULL AND storage_runtime_fingerprint IS NULL) OR
  (storage_runtime_fingerprint IS NOT NULL AND jsonb_typeof(storage_policy)='object' AND octet_length(storage_policy::text)<=2048
   AND storage_policy ?& ARRAY['node','poolID','volumeBytes']
   AND storage_policy-ARRAY['node','poolID','volumeBytes']='{}'::jsonb
   AND storage_runtime_fingerprint ~ '^[a-f0-9]{64}$'));
ALTER TABLE webdock_auth.git_build_attempt
 ADD COLUMN storage_request jsonb,
 ADD COLUMN storage_epoch bigint NOT NULL DEFAULT 0 CHECK(storage_epoch BETWEEN 0 AND 9007199254740991),
 ADD COLUMN storage_closing boolean NOT NULL DEFAULT false,
 ADD COLUMN volume_handle jsonb,
 ADD COLUMN storage_retired_at timestamptz,
 ADD CONSTRAINT git_build_attempt_storage_request CHECK(storage_request IS NULL OR (
  jsonb_typeof(storage_request)='object' AND octet_length(storage_request::text)<=4096
  AND storage_request ?& ARRAY['version','audience','volumeID','customerID','projectID','buildID','generation','target','volumeBytes','action','epoch','expiresAt']
  AND storage_request->>'customerID'=customer_id AND storage_request->>'projectID'=project_id
  AND storage_request->>'buildID'=build_id AND (storage_request->>'generation')::bigint=generation
  AND storage_request#>>'{target,clusterID}'=cluster_id
  AND storage_request->>'volumeID' ~ '^[1-9][0-9]{0,18}$'
  AND (storage_request->>'volumeID')::numeric<=9223372036854775807
  AND storage_request->>'audience'='webdock-build-storage-v1'
  AND (storage_request->>'volumeBytes')::bigint BETWEEN 67108864 AND 68719476736
  AND storage_epoch>0) IS TRUE),
 ADD CONSTRAINT git_build_attempt_volume_handle CHECK(volume_handle IS NULL OR (
  storage_request IS NOT NULL AND jsonb_typeof(volume_handle)='object' AND octet_length(volume_handle::text)<=16384
  AND volume_handle->>'namespace'=namespace
  AND volume_handle#>>'{reservation,customerID}'=customer_id AND volume_handle#>>'{reservation,projectID}'=project_id
  AND volume_handle#>>'{reservation,buildID}'=build_id AND (volume_handle#>>'{reservation,generation}')::bigint=generation
  AND volume_handle#>'{reservation,target}'=storage_request->'target'
  AND volume_handle#>'{reservation,volumeBytes}'=storage_request->'volumeBytes'
  AND volume_handle#>>'{reservation,volumeID}'=storage_request->>'volumeID'
  AND volume_handle->>'pvName'=('wd-bv-'||build_id||'-'||generation::text||'-'||(storage_request->>'volumeID'))
  AND volume_handle->>'pvcName'=volume_handle->>'pvName'
  AND volume_handle->>'pvUid' ~ '^[A-Za-z0-9_-]{1,128}$' AND volume_handle->>'pvcUid' ~ '^[A-Za-z0-9_-]{1,128}$') IS TRUE),
 ADD CONSTRAINT git_build_attempt_storage_close CHECK(storage_retired_at IS NULL OR (storage_request IS NOT NULL AND storage_closing AND cancel_requested));
CREATE UNIQUE INDEX git_build_attempt_volume_identity ON webdock_auth.git_build_attempt((storage_request->>'volumeID')) WHERE storage_request IS NOT NULL;
CREATE FUNCTION webdock_rls.guard_git_build_volume_authority() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 IF TG_OP='UPDATE' THEN
  IF (OLD.storage_request IS NOT NULL AND NEW.storage_request IS DISTINCT FROM OLD.storage_request)
   OR (OLD.storage_request IS NULL AND NEW.storage_request IS NOT NULL AND
       (OLD.spec_fingerprint IS NOT NULL OR OLD.job_uid IS NOT NULL OR OLD.cancel_requested OR NEW.storage_epoch<>1))
   OR NEW.storage_epoch<OLD.storage_epoch
   OR (OLD.storage_closing AND NOT NEW.storage_closing)
   OR (OLD.volume_handle IS NOT NULL AND NEW.volume_handle IS DISTINCT FROM OLD.volume_handle)
   OR (OLD.storage_retired_at IS NOT NULL AND NEW.storage_retired_at IS DISTINCT FROM OLD.storage_retired_at) THEN
   RAISE EXCEPTION 'volume authority is immutable or fenced' USING ERRCODE='23514';
  END IF;
 END IF;
 IF (NEW.storage_request IS NULL AND (NEW.storage_epoch<>0 OR NEW.storage_closing OR NEW.volume_handle IS NOT NULL OR NEW.storage_retired_at IS NOT NULL))
  OR (NEW.storage_request IS NOT NULL AND NEW.spec_fingerprint IS NOT NULL AND NEW.volume_handle IS NULL) THEN
  RAISE EXCEPTION 'bound storage required before job intent' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_rls.guard_git_build_volume_authority() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.guard_git_build_volume_authority() FROM PUBLIC;
CREATE TRIGGER git_build_attempt_volume BEFORE INSERT OR UPDATE ON webdock_auth.git_build_attempt
 FOR EACH ROW EXECUTE FUNCTION webdock_rls.guard_git_build_volume_authority();
