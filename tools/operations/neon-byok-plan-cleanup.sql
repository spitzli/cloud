-- One-time configuration migration after deploying the BYOK-aware plan writers.
-- Run with psql -X --set ON_ERROR_STOP=1. The JSON result is a private rollback record.
BEGIN;
SET LOCAL lock_timeout = '5s';
SET LOCAL statement_timeout = '30s';
LOCK TABLE webdock_auth.plan, webdock_auth.customer_plan IN SHARE ROW EXCLUSIVE MODE;
CREATE TEMP TABLE neon_byok_plan_before ON COMMIT DROP AS
 SELECT 'plan'::text AS kind,id AS id,limits AS value,updated_by,updated_at FROM webdock_auth.plan
 UNION ALL
 SELECT 'customer',customer_id,overrides,updated_by,updated_at FROM webdock_auth.customer_plan;
UPDATE webdock_auth.plan SET
 limits=coalesce((SELECT jsonb_object_agg(e.key,e.value) FROM jsonb_each(limits) e WHERE e.key<>'neon' AND e.key NOT LIKE 'neon.%'),'{}'::jsonb),
 updated_by='system:neon-byok-policy',updated_at=now()
 WHERE EXISTS(SELECT 1 FROM jsonb_object_keys(limits) k WHERE k='neon' OR k LIKE 'neon.%');
UPDATE webdock_auth.customer_plan SET
 overrides=coalesce((SELECT jsonb_object_agg(e.key,e.value) FROM jsonb_each(overrides) e WHERE e.key<>'neon' AND e.key NOT LIKE 'neon.%'),'{}'::jsonb),
 updated_by='system:neon-byok-policy',updated_at=now()
 WHERE EXISTS(SELECT 1 FROM jsonb_object_keys(overrides) k WHERE k='neon' OR k LIKE 'neon.%');
SELECT jsonb_build_object('before',coalesce((SELECT jsonb_agg(to_jsonb(b)) FROM neon_byok_plan_before b),'[]'::jsonb),
 'after',coalesce((SELECT jsonb_agg(v) FROM (
 SELECT jsonb_build_object('kind','plan','id',id,'value',limits,'updated_by',updated_by,'updated_at',updated_at) v FROM webdock_auth.plan
 UNION ALL
 SELECT jsonb_build_object('kind','customer','id',customer_id,'value',overrides,'updated_by',updated_by,'updated_at',updated_at) FROM webdock_auth.customer_plan) s),'[]'::jsonb));
COMMIT;
