/** Source edits can invalidate obsolete work without gaining deployment creation authority. */
export const gitInvalidationSQL = String.raw`
CREATE OR REPLACE FUNCTION webdock_rls.cancel_obsolete_git_builds() RETURNS void LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE cancelled record;
BEGIN
 FOR cancelled IN
  UPDATE webdock_auth.git_build b SET status='cancelled',generation=b.generation+1,lease_until=NULL,finished_at=now(),failure_code='SOURCE_CHANGED'
  FROM webdock_auth.git_source s JOIN webdock_auth.git_connection c ON c.id=s.connection_id
  WHERE b.source_id=s.id AND b.status IN ('queued','running')
   AND (webdock_rls.worker(ARRAY['git-worker']) OR webdock_rls.customer_permission(b.customer_id,'org:projects:manage') OR webdock_rls.customer_permission(b.customer_id,'org:deployments:create'))
   AND (NOT s.enabled OR c.state<>'active' OR s.revision<>b.source_revision OR c.generation<>b.connection_generation) RETURNING b.id
 LOOP
  INSERT INTO webdock_auth.git_check_outbox(build_id,installation_id,repository_id,source_sha,status,conclusion)
  SELECT b.id,c.installation_id,s.repository_id,b.source_sha,'completed','cancelled' FROM webdock_auth.git_build b JOIN webdock_auth.git_source s ON s.id=b.source_id JOIN webdock_auth.git_connection c ON c.id=s.connection_id WHERE b.id=cancelled.id
  ON CONFLICT(build_id) DO UPDATE SET status='completed',conclusion='cancelled',version=webdock_auth.git_check_outbox.version+1,retry_at=now();
 END LOOP;
 UPDATE webdock_auth.git_release r SET status='superseded',revision=r.revision+1 FROM webdock_auth.git_source s JOIN webdock_auth.git_connection c ON c.id=s.connection_id
 WHERE r.source_id=s.id AND r.status IN ('awaiting-approval','queued')
  AND (webdock_rls.worker(ARRAY['git-worker']) OR webdock_rls.customer_permission(r.customer_id,'org:projects:manage') OR webdock_rls.customer_permission(r.customer_id,'org:deployments:create'))
  AND (NOT s.enabled OR c.state<>'active' OR s.revision<>r.source_revision OR c.generation<>r.connection_generation);
END $$;
ALTER FUNCTION webdock_rls.cancel_obsolete_git_builds() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.cancel_obsolete_git_builds() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.cancel_obsolete_git_builds() TO webdock_auth_runtime,webdock_worker_runtime;
`;
