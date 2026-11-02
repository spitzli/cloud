-- Transactional, append-only invalidation. Writers never update a shared counter.
CREATE SCHEMA IF NOT EXISTS webdock_cache AUTHORIZATION webdock_platform_owner;
REVOKE ALL ON SCHEMA webdock_cache FROM PUBLIC;
CREATE TABLE IF NOT EXISTS webdock_cache.domains (
 name text PRIMARY KEY,
 epoch uuid NOT NULL DEFAULT gen_random_uuid()
);
CREATE TABLE IF NOT EXISTS webdock_cache.events (
 event_id bigint GENERATED ALWAYS AS IDENTITY UNIQUE,
 domain text NOT NULL REFERENCES webdock_cache.domains(name),
 transaction_id text NOT NULL,
 created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
 PRIMARY KEY(domain,transaction_id)
);
CREATE INDEX IF NOT EXISTS cache_events_created ON webdock_cache.events(created_at,event_id);
ALTER TABLE webdock_cache.domains OWNER TO webdock_platform_owner;
ALTER TABLE webdock_cache.events OWNER TO webdock_platform_owner;
REVOKE ALL ON ALL TABLES IN SCHEMA webdock_cache FROM PUBLIC;
REVOKE ALL ON ALL SEQUENCES IN SCHEMA webdock_cache FROM PUBLIC;
INSERT INTO webdock_cache.domains(name) VALUES
 ('webdock_admin.customers'),('webdock_admin.projects'),('webdock_admin.cms_instances'),
 ('webdock_admin.audit_events'),('webdock_admin.users'),
 ('webdock_auth.user'),('webdock_auth.member'),('webdock_auth.tenant_customer'),
 ('webdock_auth.hosting_app'),('webdock_auth.hosting_operation'),('webdock_auth.hosting_project'),
 ('webdock_auth.hosting_cluster'),('webdock_auth.hosting_agent'),
 ('webdock_auth.hosting_setup'),('webdock_auth.git_source'),('webdock_auth.git_repository'),
 ('webdock_auth.hosting_database'),('webdock_auth.hosting_database_backend'),
 ('webdock_auth.hosting_service_binding'),('webdock_auth.mail_api_key'),
 ('webdock_auth.mail_connection'),('webdock_auth.mail_sender_domain'),
 ('webdock_auth.platform_settings'),('webdock_auth.plan_template')
ON CONFLICT DO NOTHING;

CREATE OR REPLACE FUNCTION webdock_cache.changed() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_cache AS $$
BEGIN
 IF TG_OP <> 'TRUNCATE' THEN
  IF NOT EXISTS (SELECT 1 FROM changed_rows) THEN RETURN NULL; END IF;
 END IF;
 INSERT INTO webdock_cache.events(domain,transaction_id)
 VALUES(TG_TABLE_SCHEMA||'.'||TG_TABLE_NAME,pg_current_xact_id()::text)
 ON CONFLICT(domain,transaction_id) DO NOTHING;
 RETURN NULL;
END $$;
ALTER FUNCTION webdock_cache.changed() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_cache.changed() FROM PUBLIC;

DO $$ DECLARE item record; qualified text; BEGIN
 FOR item IN SELECT name FROM webdock_cache.domains ORDER BY name LOOP
  qualified:=format('%I.%I',split_part(item.name,'.',1),split_part(item.name,'.',2));
  IF to_regclass(qualified) IS NULL THEN RAISE EXCEPTION 'Missing cache source: %',item.name; END IF;
  EXECUTE format('DROP TRIGGER IF EXISTS webdock_cache_insert ON %s',qualified);
  EXECUTE format('DROP TRIGGER IF EXISTS webdock_cache_update ON %s',qualified);
  EXECUTE format('DROP TRIGGER IF EXISTS webdock_cache_delete ON %s',qualified);
  EXECUTE format('DROP TRIGGER IF EXISTS webdock_cache_truncate ON %s',qualified);
  EXECUTE format('CREATE TRIGGER webdock_cache_insert AFTER INSERT ON %s REFERENCING NEW TABLE AS changed_rows FOR EACH STATEMENT EXECUTE FUNCTION webdock_cache.changed()',qualified);
  EXECUTE format('CREATE TRIGGER webdock_cache_update AFTER UPDATE ON %s REFERENCING NEW TABLE AS changed_rows FOR EACH STATEMENT EXECUTE FUNCTION webdock_cache.changed()',qualified);
  EXECUTE format('CREATE TRIGGER webdock_cache_delete AFTER DELETE ON %s REFERENCING OLD TABLE AS changed_rows FOR EACH STATEMENT EXECUTE FUNCTION webdock_cache.changed()',qualified);
  EXECUTE format('CREATE TRIGGER webdock_cache_truncate AFTER TRUNCATE ON %s FOR EACH STATEMENT EXECUTE FUNCTION webdock_cache.changed()',qualified);
 END LOOP;
END $$;

CREATE OR REPLACE FUNCTION webdock_cache.version(requested text[]) RETURNS text
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog,webdock_cache AS $$
DECLARE names text[]; result text; BEGIN
 SELECT array_agg(DISTINCT name ORDER BY name) INTO names FROM unnest(requested) name;
 IF names IS NULL OR cardinality(names)>64 OR array_position(names,NULL) IS NOT NULL
 OR EXISTS(SELECT 1 FROM unnest(names) AS requested_name(name) WHERE NOT EXISTS(SELECT 1 FROM webdock_cache.domains d WHERE d.name=requested_name.name)) THEN
  RAISE EXCEPTION 'Unknown cache source';
 END IF;
 -- Never publish a view of this transaction's uncommitted source writes.
 IF EXISTS(SELECT 1 FROM webdock_cache.events e WHERE e.domain=ANY(names)
  AND e.transaction_id=pg_current_xact_id_if_assigned()::text) THEN RETURN NULL; END IF;
 -- COUNT detects a late commit whose event_id is lower than an already visible event.
 SELECT string_agg(name||':'||epoch::text||':'||events::text||':'||latest::text,'|' ORDER BY name) INTO result
 FROM (SELECT d.name,d.epoch,count(e.event_id) AS events,coalesce(max(e.event_id),0) AS latest
  FROM webdock_cache.domains d LEFT JOIN webdock_cache.events e ON e.domain=d.name
  WHERE d.name=ANY(names) GROUP BY d.name,d.epoch) versions;
 RETURN result;
END $$;
ALTER FUNCTION webdock_cache.version(text[]) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_cache.version(text[]) FROM PUBLIC;

CREATE OR REPLACE FUNCTION webdock_cache.prune() RETURNS integer
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,webdock_cache AS $$
DECLARE changed integer; BEGIN
 IF NOT pg_try_advisory_xact_lock(hashtextextended('webdock-cache-prune',0)) THEN RETURN 0; END IF;
 -- One bounded batch; removing events and rotating epochs are atomic to readers.
 WITH candidates AS (
  SELECT event_id FROM webdock_cache.events WHERE created_at<clock_timestamp()-interval '5 minutes'
  ORDER BY created_at,event_id LIMIT 10000
 ), deleted AS (
  DELETE FROM webdock_cache.events WHERE event_id IN (SELECT event_id FROM candidates) RETURNING domain
 ) UPDATE webdock_cache.domains SET epoch=gen_random_uuid() WHERE name IN(SELECT domain FROM deleted);
 GET DIAGNOSTICS changed=ROW_COUNT;
 RETURN changed;
END $$;
ALTER FUNCTION webdock_cache.prune() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_cache.prune() FROM PUBLIC;
GRANT USAGE ON SCHEMA webdock_cache TO webdock_auth_runtime,webdock_registry_runtime,webdock_worker_runtime,webdock_auth_bootstrap;
GRANT EXECUTE ON FUNCTION webdock_cache.version(text[]) TO webdock_auth_runtime,webdock_registry_runtime,webdock_worker_runtime,webdock_auth_bootstrap;
GRANT EXECUTE ON FUNCTION webdock_cache.prune() TO webdock_auth_bootstrap;
