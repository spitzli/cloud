-- Optional Core diagnostics. Run on the reviewed database using an admin client.
-- This is infrastructure setup, not a runtime role grant or an app migration.
BEGIN;
SET LOCAL lock_timeout = '3s';
SET LOCAL statement_timeout = '20s';
CREATE EXTENSION IF NOT EXISTS pg_stat_statements WITH SCHEMA public;
COMMIT;
