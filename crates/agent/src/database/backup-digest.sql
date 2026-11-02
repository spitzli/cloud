CREATE OR REPLACE FUNCTION pg_temp.webdock_restore_digest() RETURNS text LANGUAGE plpgsql AS $webdock$
DECLARE r record; part text; combined text := '';
BEGIN
 FOR r IN SELECT n.nspname,c.relname FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE c.relkind IN ('r','p') AND n.nspname !~ '^pg_' AND n.nspname<>'information_schema' ORDER BY n.nspname,c.relname LOOP
  EXECUTE format('SELECT encode(sha256(convert_to(COALESCE(string_agg(row_to_json(t)::text,E''\n'' ORDER BY row_to_json(t)::text),''''),''UTF8'')),''hex'') FROM %I.%I t',r.nspname,r.relname) INTO part;
  combined := combined || r.nspname || '.' || r.relname || ':' || part || E'\n';
 END LOOP;
 RETURN encode(sha256(convert_to(combined,'UTF8')),'hex');
END $webdock$;
BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;
SELECT pg_temp.webdock_restore_digest();
COMMIT;
