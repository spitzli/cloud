-- Additive repository authority for the existing durable archives job; no synthetic Session.
ALTER TABLE webdock_auth.repository_lease DROP CONSTRAINT repository_lease_authority_kind_check;
ALTER TABLE webdock_auth.repository_lease ADD CONSTRAINT repository_lease_authority_kind_check
 CHECK(authority_kind IN ('session','build','release','plugin-job'));
ALTER TABLE webdock_auth.repository_lease ADD COLUMN archive_authority jsonb;
ALTER TABLE webdock_auth.repository_lease ADD CONSTRAINT repository_archive_authority_bound CHECK (
 (authority_kind='plugin-job' AND archive_authority IS NOT NULL AND jsonb_typeof(archive_authority)='object' AND octet_length(archive_authority::text)<=16384)
 OR (authority_kind<>'plugin-job' AND archive_authority IS NULL));

ALTER FUNCTION webdock_rls.repository_domain_live(uuid) RENAME TO repository_domain_live_before_archives;
REVOKE ALL ON FUNCTION webdock_rls.repository_domain_live_before_archives(uuid) FROM PUBLIC,webdock_worker_runtime;
CREATE FUNCTION webdock_rls.repository_domain_live(lease_id uuid) RETURNS jsonb
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE l webdock_auth.repository_lease%ROWTYPE; v webdock_auth.plugin_invocation%ROWTYPE;
 j webdock_auth.plugin_job_execution%ROWTYPE; s webdock_auth.git_source%ROWTYPE;
 c webdock_auth.git_connection%ROWTYPE; actor jsonb; claims jsonb;
BEGIN
 IF NOT webdock_rls.worker(ARRAY['repository-admission']) THEN RETURN NULL; END IF;
 SELECT * INTO l FROM webdock_auth.repository_lease x WHERE x.id=lease_id AND x.state='active' AND x.expires_at>clock_timestamp();
 IF NOT FOUND THEN RETURN NULL; END IF;
 IF l.authority_kind<>'plugin-job' THEN RETURN webdock_rls.repository_domain_live_before_archives(lease_id); END IF;
 claims:=l.archive_authority->'claims';
 SELECT * INTO v FROM webdock_auth.plugin_invocation x WHERE x.id::text=l.authority_id
  AND x.plugin_id='archives' AND x.name='archives.produce' AND x.kind='job'
  AND x.revoked_at IS NULL AND x.expires_at>clock_timestamp()
  AND x.customer_id=l.customer_id AND x.project_id=l.project_id
  AND x.actor_id=l.actor_subject AND x.session_id=l.actor_session;
 IF NOT FOUND OR claims->>'id' IS DISTINCT FROM v.id::text OR claims->>'pluginID'<>'archives'
  OR claims->>'projectID' IS DISTINCT FROM v.project_id OR claims->>'customerID' IS DISTINCT FROM v.customer_id
  OR claims->>'action'<>'archives.produce' OR (claims->>'generation')::bigint<>v.runtime_generation
  OR to_timestamp((claims->>'exp')::bigint)<>v.expires_at OR l.expires_at>v.expires_at THEN RETURN NULL; END IF;
 SELECT * INTO j FROM webdock_auth.plugin_job_execution x WHERE x.id=v.job_execution_id
  AND x.id=claims#>>'{job,id}' AND x.attempts=l.authority_generation
  AND x.attempts=(claims#>>'{job,attempt}')::integer AND x.invocation_id=v.id
  AND x.claim_token=v.job_claim_token AND x.status='running' AND NOT x.cancel_requested
  AND x.lease_until>clock_timestamp() AND x.plugin_id=v.plugin_id
  AND x.customer_id=v.customer_id AND x.project_id=v.project_id
  AND x.runtime_generation=v.runtime_generation AND x.grant_revision=v.grant_revision;
 IF NOT FOUND OR j.provenance->>'actor_id' IS DISTINCT FROM v.actor_id
  OR j.provenance->>'session_id' IS DISTINCT FROM v.session_id
  OR j.provenance->'session_context' IS DISTINCT FROM v.session_context
  OR j.provenance#>'{session_context,authority}' IS NULL
  OR j.provenance#>'{session_context,authority}'='null'::jsonb THEN RETURN NULL; END IF;
 IF NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_installation p
  JOIN webdock_auth.plugin_runtime r ON r.plugin_id=p.id AND r.generation=p.current_generation
  JOIN webdock_auth.plugin_configuration config ON config.plugin_id=p.id
  JOIN webdock_auth.plugin_grant g ON g.plugin_id=p.id AND g.project_id=v.project_id AND g.customer_id=v.customer_id
  WHERE p.id='archives' AND p.state IN ('enabled','draining') AND p.runtime='ready'
   AND r.status IN ('ready','draining') AND r.generation=v.runtime_generation
   AND r.configuration_revision=config.revision AND g.revoked_at IS NULL
   AND g.revision=v.grant_revision AND g.jobs ? 'archives.produce') THEN RETURN NULL; END IF;
 IF NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_resource r WHERE r.plugin_id='archives'
  AND r.resource_id=j.provenance#>>'{session_context,resourceID}' AND r.project_id=v.project_id AND r.customer_id=v.customer_id)
  OR NOT webdock_rls.plugin_feature_supported('archives','exports') THEN RETURN NULL; END IF;
 actor:=webdock_rls.git_actor_authority_internal(l.customer_id,l.actor_subject,l.actor_session);
 IF actor IS NULL THEN RETURN NULL; END IF;
 IF NOT coalesce((actor->>'operator')::boolean,false) AND NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_feature_policy p
  WHERE p.plugin_id='archives' AND p.feature_id='exports' AND (p.mode='everyone' OR p.mode='selected'
   AND EXISTS(SELECT 1 FROM webdock_auth.plugin_feature_customer f WHERE f.plugin_id=p.plugin_id AND f.feature_id=p.feature_id AND f.customer_id=l.customer_id))) THEN RETURN NULL; END IF;
 SELECT * INTO s FROM webdock_auth.git_source x WHERE x.id=l.source_id AND x.project_id=l.project_id AND x.customer_id=l.customer_id AND x.enabled AND x.revision=l.source_revision;
 IF NOT FOUND THEN RETURN NULL; END IF;
 SELECT * INTO c FROM webdock_auth.git_connection x WHERE x.id=s.connection_id AND x.customer_id=s.customer_id AND x.state='active' AND x.generation=l.connection_generation AND x.repository_plugin_id=l.plugin_id;
 IF NOT FOUND OR NOT EXISTS(SELECT 1 FROM webdock_auth.git_repository q WHERE q.connection_id=c.id AND q.repository_id=s.repository_id AND q.customer_id=s.customer_id)
  OR l.input_json::jsonb->>'installationID' IS DISTINCT FROM c.installation_id
  OR l.input_json::jsonb->>'repositoryID' IS DISTINCT FROM s.repository_id THEN RETURN NULL; END IF;
 IF l.action='resolve' THEN
  IF NOT (l.input_json::jsonb->>'reference'=s.branch OR l.input_json::jsonb->>'reference'~'^[a-f0-9]{40}$') THEN RETURN NULL; END IF;
 ELSIF l.action='archive' THEN
  IF NOT coalesce(l.input_json::jsonb->>'commit'~'^[a-f0-9]{40}$',false) THEN RETURN NULL; END IF;
 ELSE RETURN NULL; END IF;
 IF NOT EXISTS(SELECT 1 FROM webdock_admin.projects p JOIN webdock_admin.customers u ON u.id=p.customer_id
  WHERE p.id=l.project_id AND p.customer_id=l.customer_id AND p.status='active' AND u.status='active'
  AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=p.id AND d.status<>'deleted'))
  OR NOT EXISTS(SELECT 1 FROM webdock_auth.plugin_installation p WHERE p.id='hosting' AND p.state IN ('enabled','draining'))
  OR NOT EXISTS(SELECT 1 FROM webdock_auth.hosting_project h WHERE h.project_id=l.project_id AND h.customer_id=l.customer_id
   AND (h.mode='selfservice' OR coalesce((actor->>'operator')::boolean,false))) THEN RETURN NULL; END IF;
 RETURN actor;
END $$;
ALTER FUNCTION webdock_rls.repository_domain_live(uuid) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.repository_domain_live(uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.repository_domain_live(uuid) TO webdock_worker_runtime;
