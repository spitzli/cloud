-- Privileged operator inspection only. Never supplies an interactive/user RLS context.
-- Run with an authorized read-only inspection connection; fail if its grants are insufficient.
\set ON_ERROR_STOP on
BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY;
-- Error instead of silently exporting an RLS-filtered, incomplete reference set.
SET LOCAL row_security=off;
SET LOCAL statement_timeout='30s';
SET LOCAL idle_in_transaction_session_timeout='30s';
SELECT jsonb_build_object(
 'version',1,'capturedAt',floor(extract(epoch FROM clock_timestamp()))::bigint,
 'builds',coalesce((SELECT jsonb_agg(jsonb_build_object('id',id,'customerID',customer_id,'generation',generation,'status',status)) FROM webdock_auth.git_build),'[]'::jsonb),
 'references',coalesce((SELECT jsonb_agg(jsonb_build_object('buildID',build_id,'customerID',customer_id,'receipt',remote_storage)) FROM webdock_auth.git_artifact),'[]'::jsonb)
);
COMMIT;
