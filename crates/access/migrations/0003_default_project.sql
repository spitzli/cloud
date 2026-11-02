-- Native0029: every customer starts with a Default project. Archiving or deleting a customer takes
-- that project along as long as nothing lives in it.
ALTER TABLE webdock_admin.projects ADD COLUMN created_default boolean NOT NULL DEFAULT false;

CREATE FUNCTION webdock_admin.create_default_project() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 INSERT INTO webdock_admin.projects(id,name,customer_id,created_default) VALUES(webdock_admin.next_snowflake(),'Default',NEW.id,true);
 RETURN NEW;
END $$;
ALTER FUNCTION webdock_admin.create_default_project() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_admin.create_default_project() FROM PUBLIC;
CREATE TRIGGER webdock_default_project AFTER INSERT ON webdock_admin.customers
 FOR EACH ROW EXECUTE FUNCTION webdock_admin.create_default_project();

INSERT INTO webdock_admin.projects(id,name,customer_id,created_default)
 SELECT webdock_admin.next_snowflake(),'Default',c.id,true FROM webdock_admin.customers c
 WHERE c.status='active' AND NOT EXISTS(SELECT 1 FROM webdock_admin.projects p WHERE p.customer_id=c.id);

-- Empty: no row in any table that references the project.
CREATE FUNCTION webdock_admin.project_is_empty(project text) RETURNS boolean
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE r record; found boolean;
BEGIN
 FOR r IN SELECT c.conrelid::regclass AS tab, a.attname AS col FROM pg_constraint c
   JOIN pg_attribute a ON a.attrelid=c.conrelid AND a.attnum=c.conkey[1]
   WHERE c.contype='f' AND c.confrelid='webdock_admin.projects'::regclass LOOP
  EXECUTE format('SELECT EXISTS(SELECT 1 FROM %s WHERE %I=$1)', r.tab, r.col) INTO found USING project;
  IF found THEN RETURN false; END IF;
 END LOOP;
 RETURN true;
END $$;
ALTER FUNCTION webdock_admin.project_is_empty(text) OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_admin.project_is_empty(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION webdock_admin.project_is_empty(text) TO webdock_registry_runtime;

-- Deleting a customer deletes its Default project while it is empty; anything else still blocks.
CREATE FUNCTION webdock_admin.delete_default_project() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
 DELETE FROM webdock_admin.projects WHERE customer_id=OLD.id AND created_default AND webdock_admin.project_is_empty(id);
 RETURN OLD;
END $$;
ALTER FUNCTION webdock_admin.delete_default_project() OWNER TO webdock_platform_owner;
REVOKE ALL ON FUNCTION webdock_admin.delete_default_project() FROM PUBLIC;
CREATE TRIGGER webdock_default_project_delete BEFORE DELETE ON webdock_admin.customers
 FOR EACH ROW EXECUTE FUNCTION webdock_admin.delete_default_project();
