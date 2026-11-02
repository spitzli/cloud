-- Native0021: availability transitions must preserve already accepted domain work.
-- The narrow result exposes no cross-domain rows or caller-selected worker purpose.
CREATE FUNCTION webdock_rls.plugin_state_continuations(operation_id text, claim uuid) RETURNS boolean
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE plugin text;
BEGIN
 IF NOT webdock_rls.worker(ARRAY['plugin-controller']::text[]) THEN RETURN NULL; END IF;
 PERFORM pg_advisory_xact_lock(hashtext('webdock-plugin-installation'));
 SELECT p.id INTO plugin
 FROM webdock_auth.plugin_installation p
 JOIN webdock_auth.plugin_operation o ON o.plugin_id=p.id
 WHERE o.id=operation_id AND o.claim_token=claim AND o.status='running'
  AND o.lease_until>clock_timestamp() AND o.generation=p.generation
  AND p.kind='external' AND p.state='draining' AND p.desired_state='disabled'
  AND o.payload->>'desired'='disabled'
 FOR UPDATE OF p,o;
 IF NOT FOUND THEN RETURN NULL; END IF;
 -- Callers keep the installation lock through the final state update. Domain
 -- admissions/continuations take the matching shared lock before queuing or handoff.
 RETURN EXISTS(SELECT 1 FROM webdock_auth.plugin_invocation v
  WHERE v.plugin_id=plugin AND v.revoked_at IS NULL AND v.expires_at>clock_timestamp())
 OR EXISTS(SELECT 1 FROM webdock_auth.plugin_job_execution j
  WHERE j.plugin_id=plugin AND j.status IN ('pending','queued','running'))
 OR EXISTS(SELECT 1 FROM webdock_mail.send_job j
  WHERE (j.route->>'pluginID'=plugin OR (
    j.route->>'pluginID' IS NULL AND EXISTS(
     SELECT 1 FROM webdock_mail.provider_binding b WHERE b.customer_id=j.customer_id AND b.plugin_id=plugin)))
   AND j.state IN ('staging','queued','leased','preparation_review','handoff','unknown'))
 OR EXISTS(SELECT 1 FROM webdock_mail.provider_lease l
  WHERE l.plugin_id=plugin AND l.revoked_at IS NULL AND l.expires_at>clock_timestamp())
 OR EXISTS(SELECT 1 FROM webdock_auth.repository_lease l
  WHERE l.plugin_id=plugin AND l.state='active' AND l.expires_at>clock_timestamp())
 OR EXISTS(SELECT 1 FROM webdock_auth.git_build b
  JOIN webdock_auth.git_source s ON s.id=b.source_id
  JOIN webdock_auth.git_connection c ON c.id=s.connection_id
  WHERE c.repository_plugin_id=plugin AND b.status IN ('queued','running'))
 OR EXISTS(SELECT 1 FROM webdock_auth.git_release r
  JOIN webdock_auth.git_source s ON s.id=r.source_id
  JOIN webdock_auth.git_connection c ON c.id=s.connection_id
  WHERE c.repository_plugin_id=plugin AND r.status IN ('deploying','needs-reconciliation'));
END $$;
ALTER FUNCTION webdock_rls.plugin_state_continuations(text,uuid) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.plugin_state_continuations(text,uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.plugin_state_continuations(text,uuid) TO webdock_worker_runtime;
