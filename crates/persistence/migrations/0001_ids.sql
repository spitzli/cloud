-- Offline migration; never execute from a request or ordinary runtime startup.
CREATE SCHEMA IF NOT EXISTS webdock_ids;
CREATE TABLE IF NOT EXISTS webdock_ids.generator (
 slot smallint PRIMARY KEY CHECK (slot BETWEEN 2 AND 1023),
 layout text NOT NULL CHECK (layout='2026-01-01:41:6:4:12'),
 high_water bigint CHECK (high_water > 0 AND ((high_water >> 12) & 1023) = slot)
);
INSERT INTO webdock_ids.generator(slot,layout) VALUES(2,'2026-01-01:41:6:4:12') ON CONFLICT DO NOTHING;
REVOKE ALL ON SCHEMA webdock_ids FROM PUBLIC;
REVOKE ALL ON webdock_ids.generator FROM PUBLIC;
CREATE OR REPLACE FUNCTION webdock_ids.enforce_high_water() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog AS $$ BEGIN
 IF NEW.slot<>OLD.slot OR NEW.layout<>OLD.layout OR NEW.high_water IS NULL
    OR (OLD.high_water IS NOT NULL AND NEW.high_water<=OLD.high_water) THEN
  RAISE EXCEPTION 'Generator identity/layout and committed high-water are immutable' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS generator_high_water ON webdock_ids.generator;
CREATE TRIGGER generator_high_water BEFORE UPDATE ON webdock_ids.generator
 FOR EACH ROW EXECUTE FUNCTION webdock_ids.enforce_high_water();
REVOKE ALL ON FUNCTION webdock_ids.enforce_high_water() FROM PUBLIC;
DO $$ DECLARE role_name text; BEGIN
 IF EXISTS(SELECT FROM pg_roles WHERE rolname='webdock_platform_owner') THEN
  ALTER SCHEMA webdock_ids OWNER TO webdock_platform_owner;
  ALTER TABLE webdock_ids.generator OWNER TO webdock_platform_owner;
  ALTER FUNCTION webdock_ids.enforce_high_water() OWNER TO webdock_platform_owner;
 END IF;
 FOREACH role_name IN ARRAY ARRAY['webdock_auth_bootstrap','webdock_auth_runtime','webdock_registry_runtime','webdock_worker_runtime'] LOOP
  IF EXISTS(SELECT FROM pg_roles WHERE rolname=role_name) THEN
   EXECUTE format('GRANT USAGE ON SCHEMA webdock_ids TO %I',role_name);
   EXECUTE format('GRANT SELECT,UPDATE(high_water) ON webdock_ids.generator TO %I',role_name);
  END IF;
 END LOOP;
END $$;
