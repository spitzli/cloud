-- Explicit container strategy settings; legacy VM recipe behavior is unchanged.
ALTER TABLE webdock_auth.git_source ADD COLUMN container_build jsonb NOT NULL DEFAULT '{"strategy":"auto","contextDirectory":".","dockerfile":"Dockerfile","buildArgs":{}}';
ALTER TABLE webdock_auth.git_source ADD CONSTRAINT git_source_container_build_check CHECK (
 jsonb_typeof(container_build)='object' AND octet_length(container_build::text)<=20000
 AND container_build ?& ARRAY['strategy','contextDirectory','dockerfile','buildArgs']
 AND container_build-ARRAY['strategy','contextDirectory','dockerfile','buildArgs']='{}'::jsonb
 AND container_build->>'strategy' IN ('auto','dockerfile','railpack')
 AND jsonb_typeof(container_build->'contextDirectory')='string'
 AND jsonb_typeof(container_build->'dockerfile')='string'
 AND jsonb_typeof(container_build->'buildArgs')='object'
);
-- The immutable execution snapshot may now carry only this explicit nonsecret container config.
ALTER TABLE webdock_auth.git_build_attempt DROP CONSTRAINT git_build_attempt_execution_request_check;
ALTER TABLE webdock_auth.git_build_attempt ADD CONSTRAINT git_build_attempt_execution_request_check CHECK (
 jsonb_typeof(execution_request)='object' AND octet_length(execution_request::text)<=32768
 AND execution_request ?& ARRAY['recipe','rootDirectory','limits']
 AND execution_request-ARRAY['recipe','rootDirectory','limits','containerBuild']='{}'::jsonb
 AND execution_request->>'recipe' IN ('dockerfile','dockerfile-frontend')
 AND jsonb_typeof(execution_request->'rootDirectory')='string'
 AND (NOT execution_request ? 'containerBuild' OR jsonb_typeof(execution_request->'containerBuild')='object')
 AND jsonb_typeof(execution_request->'limits')='object'
 AND (execution_request->'limits') ?& ARRAY['cpu','memoryMiB','diskBytes','durationSeconds']
 AND (execution_request->'limits')-ARRAY['cpu','memoryMiB','diskBytes','durationSeconds']='{}'::jsonb
 AND jsonb_typeof(execution_request#>'{limits,cpu}')='number'
 AND jsonb_typeof(execution_request#>'{limits,memoryMiB}')='number'
 AND jsonb_typeof(execution_request#>'{limits,diskBytes}')='number'
 AND jsonb_typeof(execution_request#>'{limits,durationSeconds}')='number'
 AND (execution_request#>>'{limits,cpu}')::numeric BETWEEN 1 AND 8
 AND (execution_request#>>'{limits,memoryMiB}')::numeric BETWEEN 512 AND 16384
 AND (execution_request#>>'{limits,diskBytes}')::numeric BETWEEN 10000000 AND 34359738368
 AND (execution_request#>>'{limits,durationSeconds}')::numeric BETWEEN 30 AND 1800
);
