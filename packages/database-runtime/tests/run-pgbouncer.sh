#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
fixture="webdock-scopedpool-test-${RANDOM}-$$"
cleanup() { docker rm -f "$fixture" >/dev/null 2>&1 || true; }
trap cleanup EXIT
# All credentials below belong only to this disposable loopback fixture.
docker run -d --name "$fixture" -e POSTGRES_PASSWORD=scopedpool-local-only -p 127.0.0.1::6432 postgres:18.6 >/dev/null
docker exec "$fixture" sh -c 'apt-get update -qq && apt-get install -y -qq pgbouncer >/tmp/install.log 2>&1'
for attempt in {1..30}; do
  if docker exec "$fixture" pg_isready -U postgres >/dev/null; then break; fi
  sleep 1
done
docker exec "$fixture" psql -U postgres -v ON_ERROR_STOP=1 -c 'CREATE DATABASE scopedpool_fixture' >/dev/null
docker exec -i "$fixture" psql -U postgres -d scopedpool_fixture -v ON_ERROR_STOP=1 < packages/database-runtime/tests/pgbouncer.sql >/dev/null
docker exec -i "$fixture" sh -c 'cat > /tmp/pgbouncer-test.ini' <<'CONFIG'
[databases]
scopedpool_fixture = host=127.0.0.1 port=5432 dbname=scopedpool_fixture
[pgbouncer]
listen_addr = 0.0.0.0
listen_port = 6432
unix_socket_dir = /tmp
pidfile = /tmp/pgbouncer-test.pid
logfile = /tmp/pgbouncer-test.log
auth_type = plain
auth_file = /tmp/pgbouncer-users.txt
pool_mode = transaction
default_pool_size = 1
max_db_connections = 1
max_client_conn = 50
max_prepared_statements = 100
CONFIG
docker exec -i "$fixture" sh -c 'cat > /tmp/pgbouncer-users.txt' <<'USERS'
"scoped_runtime" "runtime-local-only"
USERS
docker exec "$fixture" chown postgres:postgres /tmp/pgbouncer-test.ini /tmp/pgbouncer-users.txt
docker exec -u postgres "$fixture" pgbouncer -d /tmp/pgbouncer-test.ini
port="$(docker port "$fixture" 6432/tcp | cut -d: -f2)"
export SCOPED_POOL_TEST_URL="postgres://scoped_runtime:runtime-local-only@127.0.0.1:${port}/scopedpool_fixture"
docker exec "$fixture" pgbouncer --version
bun test ./packages/database-runtime/tests/pgbouncer.integration.ts
