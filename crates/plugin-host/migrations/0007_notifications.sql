-- Native0033: Core notices from the agent's reports when a unit needs attention: it keeps crashing, its
-- image cannot be pulled, the cluster refused it, or its certificate is not issued. One open notice per
-- unit and kind; it resolves itself once the report no longer shows it. Owners and admins of the
-- workspace get one mail per notice once it has lasted a while, so short blips stay quiet.
CREATE TABLE webdock_auth.notification (
 id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
 customer_id text NOT NULL REFERENCES webdock_admin.customers(id) ON DELETE CASCADE,
 project_id text NOT NULL,
 plugin_id text NOT NULL,
 unit text NOT NULL,
 resource_id text,
 kind text NOT NULL CHECK (kind IN ('crashing','image','refused','certificate')),
 opened_at timestamptz NOT NULL DEFAULT statement_timestamp(),
 resolved_at timestamptz,
 mailed_at timestamptz
);
CREATE UNIQUE INDEX notification_open ON webdock_auth.notification(plugin_id, unit, kind) WHERE resolved_at IS NULL;
CREATE INDEX notification_customer ON webdock_auth.notification(customer_id, opened_at DESC);
ALTER TABLE webdock_auth.notification OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.notification ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.notification FORCE ROW LEVEL SECURITY;
REVOKE ALL ON TABLE webdock_auth.notification FROM PUBLIC;
CREATE POLICY platform_owner ON webdock_auth.notification TO webdock_platform_owner USING (true) WITH CHECK (true);

-- How long a notice must last before anyone hears of it; short blips stay quiet.
CREATE FUNCTION webdock_auth.notice_delay(kind text) RETURNS interval
LANGUAGE sql IMMUTABLE SET search_path=pg_catalog AS $$
 SELECT CASE WHEN kind='certificate' THEN interval '15 minutes' ELSE interval '2 minutes' END
$$;
ALTER FUNCTION webdock_auth.notice_delay(text) OWNER TO webdock_platform_owner;

-- What a report says needs attention.
CREATE FUNCTION webdock_auth.attention(state jsonb, failure text) RETURNS text[]
LANGUAGE sql IMMUTABLE SET search_path=pg_catalog AS $$
 SELECT array_remove(ARRAY[
  CASE WHEN failure IS NOT NULL THEN 'refused' END,
  CASE WHEN EXISTS(SELECT 1 FROM jsonb_array_elements(coalesce(state->'problems','[]')) p WHERE p->>'reason' IN ('ErrImagePull','ImagePullBackOff','InvalidImageName')) THEN 'image' END,
  CASE WHEN EXISTS(SELECT 1 FROM jsonb_array_elements(coalesce(state->'problems','[]')) p WHERE p->>'reason'='CrashLoopBackOff') THEN 'crashing' END,
  CASE WHEN EXISTS(SELECT 1 FROM jsonb_array_elements(coalesce(state->'objects','[]')) o WHERE o->>'kind' LIKE '%/Certificate' AND (o->>'ready')::boolean IS NOT TRUE) THEN 'certificate' END
 ], NULL)
$$;
ALTER FUNCTION webdock_auth.attention(jsonb,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.attention(jsonb,text) FROM PUBLIC;

-- Opens and resolves a unit's notices from one report; a removed unit resolves all of them.
CREATE FUNCTION webdock_auth.notice(w webdock_auth.plugin_workload, kinds text[]) RETURNS void
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE customer text;
BEGIN
 UPDATE webdock_auth.notification SET resolved_at=statement_timestamp()
  WHERE plugin_id=w.plugin_id AND unit=w.unit AND resolved_at IS NULL AND NOT kind=ANY(kinds);
 IF cardinality(kinds)=0 THEN RETURN; END IF;
 SELECT customer_id INTO customer FROM webdock_admin.projects WHERE id=w.project_id;
 INSERT INTO webdock_auth.notification(customer_id,project_id,plugin_id,unit,resource_id,kind)
  SELECT customer,w.project_id,w.plugin_id,w.unit,w.resource_id,k FROM unnest(kinds) k
  ON CONFLICT (plugin_id,unit,kind) WHERE resolved_at IS NULL DO NOTHING;
END $$;
ALTER FUNCTION webdock_auth.notice(webdock_auth.plugin_workload,text[]) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_auth.notice(webdock_auth.plugin_workload,text[]) FROM PUBLIC;

CREATE OR REPLACE FUNCTION webdock_rls.report_plugin_workload(plugin text, workload text, reported bigint, state jsonb, failure text, removed boolean) RETURNS boolean
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE w webdock_auth.plugin_workload; cpu bigint; memory bigint; customer text;
BEGIN
 SELECT * INTO w FROM webdock_auth.plugin_workload WHERE plugin_id=plugin AND unit=workload FOR UPDATE;
 IF NOT FOUND OR NOT webdock_rls.worker(ARRAY['agent']::text[], w.cluster_id::text) OR reported>w.revision OR reported<1 THEN RETURN false; END IF;
 IF removed THEN
  IF NOT w.removing OR reported<>w.revision THEN RETURN false; END IF;
  PERFORM webdock_auth.notice(w, ARRAY[]::text[]);
  DELETE FROM webdock_auth.plugin_workload WHERE plugin_id=plugin AND unit=workload;
  RETURN true;
 END IF;
 IF state IS NULL OR jsonb_typeof(state)<>'object' OR octet_length(state::text)>65536 THEN RETURN false; END IF;
 UPDATE webdock_auth.plugin_workload SET applied_revision=greatest(applied_revision,reported), observed=state,
  error=left(failure,500), observed_at=statement_timestamp() WHERE plugin_id=plugin AND unit=workload;
 -- Only reports of the current revision speak for the unit; a removal in progress needs no attention.
 IF reported=w.revision AND NOT w.removing THEN
  PERFORM webdock_auth.notice(w, webdock_auth.attention(state, failure));
 END IF;
 IF state->'usage'->>'cpu' ~ '^[0-9]{1,12}$' AND state->'usage'->>'memory' ~ '^[0-9]{1,12}$' THEN
  cpu:=(state->'usage'->>'cpu')::bigint; memory:=(state->'usage'->>'memory')::bigint;
  SELECT customer_id INTO customer FROM webdock_admin.projects WHERE id=w.project_id;
  INSERT INTO webdock_auth.workload_usage(plugin_id,unit,customer_id,project_id,hour,samples,cpu_sum,cpu_max,memory_max)
   VALUES(plugin,workload,customer,w.project_id,date_trunc('hour',statement_timestamp()),1,cpu,cpu,memory)
   ON CONFLICT (plugin_id,unit,hour) DO UPDATE SET samples=webdock_auth.workload_usage.samples+1,
    cpu_sum=webdock_auth.workload_usage.cpu_sum+EXCLUDED.cpu_sum, cpu_max=greatest(webdock_auth.workload_usage.cpu_max,EXCLUDED.cpu_max),
    memory_max=greatest(webdock_auth.workload_usage.memory_max,EXCLUDED.memory_max);
  DELETE FROM webdock_auth.workload_usage WHERE plugin_id=plugin AND unit=workload AND hour<statement_timestamp()-interval '35 days';
 END IF;
 RETURN true;
END $$;

-- Notices due for mail: open for 2 minutes (certificates: 15), not mailed yet, with the workspace's
-- owners and admins. Only the plugin controller's mail loop asks.
CREATE FUNCTION webdock_rls.due_notifications(batch integer) RETURNS jsonb
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 IF NOT webdock_rls.worker(ARRAY['plugin-controller']::text[]) THEN RAISE EXCEPTION 'plugin controller required' USING ERRCODE='42501'; END IF;
 RETURN coalesce((SELECT jsonb_agg(jsonb_build_object('id',n.id::text,'customer',n.customer_id,'kind',n.kind,
   'emails',(SELECT coalesce(jsonb_agg(DISTINCT u.email),'[]'::jsonb) FROM webdock_auth.tenant_customer t
     JOIN webdock_auth.member m ON m."organizationId"=t.organization_id AND m.role IN ('owner','admin')
     JOIN webdock_auth."user" u ON u.id=m."userId" AND u."emailVerified" WHERE t.customer_id=n.customer_id)))
  FROM (SELECT * FROM webdock_auth.notification WHERE resolved_at IS NULL AND mailed_at IS NULL
        AND opened_at<statement_timestamp()-webdock_auth.notice_delay(kind)
        ORDER BY opened_at LIMIT least(greatest(batch,1),50)) n),'[]'::jsonb);
END $$;
ALTER FUNCTION webdock_rls.due_notifications(integer) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.due_notifications(integer) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.due_notifications(integer) TO webdock_worker_runtime;

CREATE FUNCTION webdock_rls.notification_mailed(notice bigint) RETURNS void
LANGUAGE plpgsql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 IF NOT webdock_rls.worker(ARRAY['plugin-controller']::text[]) THEN RAISE EXCEPTION 'plugin controller required' USING ERRCODE='42501'; END IF;
 UPDATE webdock_auth.notification SET mailed_at=statement_timestamp() WHERE id=notice;
END $$;
ALTER FUNCTION webdock_rls.notification_mailed(bigint) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.notification_mailed(bigint) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.notification_mailed(bigint) TO webdock_worker_runtime;

-- A workspace's notices that lasted long enough to matter: open ones, and those resolved in the last
-- 7 days. With resource and project names.
CREATE FUNCTION webdock_rls.customer_notifications(customer text) RETURNS jsonb
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 IF customer IS NULL OR NOT webdock_rls.customer_allowed(customer) THEN RAISE EXCEPTION 'customer access required' USING ERRCODE='42501'; END IF;
 RETURN coalesce((SELECT jsonb_agg(jsonb_build_object('id',n.id::text,'kind',n.kind,'plugin',n.plugin_id,'unit',n.unit,
   'project',jsonb_build_object('id',p.id,'name',p.name),
   'resource',CASE WHEN r.resource_id IS NOT NULL THEN jsonb_build_object('id',r.resource_id,'name',r.name) END,
   'openedAt',n.opened_at,'resolvedAt',n.resolved_at) ORDER BY n.resolved_at IS NOT NULL, n.opened_at DESC)
  FROM webdock_auth.notification n JOIN webdock_admin.projects p ON p.id=n.project_id
  LEFT JOIN webdock_auth.plugin_resource r ON r.plugin_id=n.plugin_id AND r.resource_id=n.resource_id AND r.project_id=n.project_id
  WHERE n.customer_id=customer AND coalesce(n.resolved_at,statement_timestamp())-n.opened_at>=webdock_auth.notice_delay(n.kind)
   AND (n.resolved_at IS NULL OR n.resolved_at>statement_timestamp()-interval '7 days')),'[]'::jsonb);
END $$;
ALTER FUNCTION webdock_rls.customer_notifications(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.customer_notifications(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.customer_notifications(text) TO webdock_auth_runtime;
