-- Retained local references stay local. S3 carries bytes, never release authority.
ALTER TABLE webdock_auth.git_artifact ADD COLUMN remote_storage jsonb;
ALTER TABLE webdock_auth.git_artifact ADD CONSTRAINT git_artifact_remote_storage_check CHECK (
 remote_storage IS NULL OR (
  jsonb_typeof(remote_storage)='object'
  AND remote_storage ?& ARRAY['backendId','binding','objectKey','sha256','sizeBytes']
  AND remote_storage->>'backendId' ~ '^[A-Za-z0-9_.-]{1,128}$'
  AND remote_storage->>'binding' ~ '^[a-f0-9]{64}$'
  AND remote_storage->>'sha256' = substring(digest FROM 8)
  AND jsonb_typeof(remote_storage->'sizeBytes')='number'
  AND (remote_storage->>'sizeBytes')::numeric=size_bytes
  AND length(remote_storage->>'objectKey') BETWEEN 1 AND 512
  AND kind='oci'
 )
);

-- Once recorded, artifact identity must survive config changes and release rollbacks.
CREATE FUNCTION webdock_rls.guard_git_artifact_identity() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
 IF ROW(NEW.build_id,NEW.customer_id,NEW.project_id,NEW.kind,NEW.digest,NEW.storage_key,
        NEW.size_bytes,NEW.image_digest,NEW.remote_storage)
    IS DISTINCT FROM
    ROW(OLD.build_id,OLD.customer_id,OLD.project_id,OLD.kind,OLD.digest,OLD.storage_key,
        OLD.size_bytes,OLD.image_digest,OLD.remote_storage) THEN
  RAISE EXCEPTION 'artifact identity is immutable' USING ERRCODE='42501';
 END IF;
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_rls.guard_git_artifact_identity() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.guard_git_artifact_identity() FROM PUBLIC;
CREATE TRIGGER git_artifact_identity BEFORE UPDATE ON webdock_auth.git_artifact
 FOR EACH ROW EXECUTE FUNCTION webdock_rls.guard_git_artifact_identity();
