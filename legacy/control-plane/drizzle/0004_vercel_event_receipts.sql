CREATE TABLE webdock_auth.provider_event_receipt (
 provider text NOT NULL CHECK(provider='vercel'), event_id text NOT NULL,
 body_hash text NOT NULL CHECK(body_hash ~ '^[a-f0-9]{64}$'), result jsonb NOT NULL,
 applied_at timestamptz NOT NULL DEFAULT now(), PRIMARY KEY(provider,event_id)
);
ALTER TABLE webdock_auth.provider_event_receipt OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth.provider_event_receipt ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth.provider_event_receipt FORCE ROW LEVEL SECURITY;
REVOKE ALL ON webdock_auth.provider_event_receipt FROM PUBLIC;
CREATE POLICY provider_event_owner ON webdock_auth.provider_event_receipt TO webdock_platform_owner USING(true) WITH CHECK(true);
GRANT SELECT ON webdock_auth.provider_event_receipt TO webdock_auth_runtime;
CREATE POLICY provider_event_operator ON webdock_auth.provider_event_receipt FOR SELECT TO webdock_auth_runtime USING(webdock_rls.operator());
GRANT USAGE,CREATE ON SCHEMA webdock_rls TO webdock_platform_owner;
CREATE FUNCTION webdock_rls.vercel_event_access(team text,project text,event text,body_hash text) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE receipt record; connection record;
BEGIN
 IF NOT webdock_rls.worker(ARRAY['vercel-event:'||team||':'||project]) THEN RAISE EXCEPTION 'Event authority required' USING ERRCODE='42501'; END IF;
 SELECT * INTO receipt FROM webdock_auth.provider_event_receipt r WHERE r.provider='vercel' AND r.event_id=event;
 IF FOUND THEN
  IF receipt.body_hash IS DISTINCT FROM body_hash THEN RAISE EXCEPTION 'Conflicting event body' USING ERRCODE='23505'; END IF;
  RETURN jsonb_build_object('receipt',receipt.result);
 END IF;
 SELECT v.encrypted_token,v.connected_by INTO connection FROM webdock_admin.vercel_connection v
 JOIN webdock_admin.users n ON n.id=v.connected_by JOIN webdock_auth."user" u ON u.id=n.auth_subject
 WHERE v.team_id=team AND n.role='operator' AND u.role='operator' AND u."twoFactorEnabled" AND u."emailVerified" AND NOT u."mustChangePassword" AND NOT coalesce(u.banned,false);
 IF NOT FOUND THEN RETURN NULL; END IF;
 RETURN jsonb_build_object('encrypted',connection.encrypted_token,'owner',connection.connected_by);
END $$;
CREATE FUNCTION webdock_rls.record_vercel_event(team text,project text,owner_id text,changed boolean,event text,body_hash text) RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_rls AS $$
DECLARE receipt record; inserted text; result jsonb;
BEGIN
 IF NOT webdock_rls.worker(ARRAY['vercel-event:'||team||':'||project]) THEN RAISE EXCEPTION 'Event authority required' USING ERRCODE='42501'; END IF;
 IF NOT EXISTS(SELECT 1 FROM webdock_admin.vercel_connection v JOIN webdock_admin.users n ON n.id=v.connected_by JOIN webdock_auth."user" u ON u.id=n.auth_subject WHERE v.team_id=team AND v.connected_by=owner_id AND n.role='operator' AND u.role='operator' AND u."twoFactorEnabled" AND u."emailVerified" AND NOT u."mustChangePassword" AND NOT coalesce(u.banned,false)) THEN RAISE EXCEPTION 'Connection ownership changed' USING ERRCODE='42501'; END IF;
 result:=jsonb_build_object('projectID',project,'region','fra1','changed',changed);
 INSERT INTO webdock_auth.provider_event_receipt(provider,event_id,body_hash,result) VALUES('vercel',event,body_hash,result) ON CONFLICT DO NOTHING RETURNING event_id INTO inserted;
 IF inserted IS NULL THEN
  SELECT * INTO receipt FROM webdock_auth.provider_event_receipt r WHERE r.provider='vercel' AND r.event_id=event;
  IF receipt.body_hash IS DISTINCT FROM body_hash THEN RAISE EXCEPTION 'Conflicting event body' USING ERRCODE='23505'; END IF;
  RETURN receipt.result;
 END IF;
 INSERT INTO webdock_admin.audit_events(id,actor_id,action,target_collection,target_i_d,summary,changed_fields)
 VALUES(webdock_admin.next_snowflake(),owner_id,'update','integrations',project,CASE WHEN changed THEN 'Vercel project region set to fra1; automatic regional failover disabled.' ELSE 'Vercel project region already fra1; automatic regional failover disabled.' END,'resourceConfig');
 RETURN result;
END $$;
ALTER FUNCTION webdock_rls.vercel_event_access(text,text,text,text) OWNER TO webdock_platform_owner;
ALTER FUNCTION webdock_rls.record_vercel_event(text,text,text,boolean,text,text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_rls.vercel_event_access(text,text,text,text),webdock_rls.record_vercel_event(text,text,text,boolean,text,text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_rls.vercel_event_access(text,text,text,text),webdock_rls.record_vercel_event(text,text,text,boolean,text,text) TO webdock_worker_runtime;
REVOKE CREATE ON SCHEMA webdock_rls FROM webdock_platform_owner;
