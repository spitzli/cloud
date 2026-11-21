#!/usr/bin/env python3
"""Disposable PostgreSQL acceptance of forward-only waitlist budget migration.

Uses a fresh network-disabled container and the real SQL migrations. Never reads
an environment database URL or connects to an existing PostgreSQL instance.
"""
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import json
import subprocess
import threading
import time
import uuid

ROOT = Path(__file__).resolve().parents[2]
IMAGE = "postgres@sha256:74935e72241653ca55e0414067e6d8763aceb8a810eb51b452253ec3dcfc4336"
MIGRATIONS = ROOT / "crates/persistence/migrations"


def run():
    name = "spitzli-waitlist-budget-" + uuid.uuid4().hex[:16]
    container = None

    def docker(*args, data=None):
        result = subprocess.run(["docker", "--host", "unix:///var/run/docker.sock", *args], input=data, text=True,
                                capture_output=True, timeout=60)
        if result.returncode:
            raise RuntimeError(f"Local waitlist fixture {args[0]} failed: {result.stderr[:1000]}")
        return result.stdout.strip()

    def sql(query):
        return docker("exec", "-i", container, "psql", "-X", "-qAt",
                      "-v", "ON_ERROR_STOP=1", "-U", "postgres", "-d", "postgres", data=query)

    def submit(identifier, email):
        assert identifier.isdigit() and email.replace("-", "").replace("@", "").replace(".", "").isalnum()
        return sql("SET ROLE webdock_auth_bootstrap; "
                   f"SELECT webdock_auth.submit_waitlist('{identifier}','{email}',NULL);") == "t"

    def reset():
        sql('TRUNCATE webdock_auth.waitlist_entry, webdock_auth."rateLimit";')

    def counts():
        return json.loads(sql('SELECT json_build_array((SELECT count(*) FROM webdock_auth.waitlist_entry),'
                              '(SELECT count FROM webdock_auth."rateLimit" WHERE key=\'waitlist:global\'));'))

    def concurrent(emails, start):
        barrier = threading.Barrier(len(emails))

        def attempt(item):
            index, email = item
            barrier.wait(timeout=20)
            return submit(str(start + index), email)

        with ThreadPoolExecutor(max_workers=len(emails)) as executor:
            return list(executor.map(attempt, enumerate(emails)))

    try:
        docker("image", "inspect", IMAGE)
        container = docker("create", "--name", name, "--label", f"spitzli.waitlist-fixture={name}",
                           "--network", "none", "--read-only", "--cap-drop", "ALL",
                           "--security-opt", "no-new-privileges", "--user", "999:999",
                           "--cpus", "1", "--memory", "256m", "--pids-limit", "128",
                           "--tmpfs", "/var/lib/postgresql:rw,uid=999,gid=999,size=256m,mode=700",
                           "--tmpfs", "/var/run/postgresql:rw,uid=999,gid=999,size=8m",
                           "--tmpfs", "/tmp:rw,uid=999,gid=999,size=8m",
                           "-e", "POSTGRES_HOST_AUTH_METHOD=trust", IMAGE)
        docker("start", container)
        for _ in range(60):
            result = subprocess.run(["docker", "--host", "unix:///var/run/docker.sock", "exec", container, "pg_isready", "-h", "127.0.0.1", "-U", "postgres"],
                                    capture_output=True, timeout=5)
            if result.returncode == 0:
                break
            time.sleep(0.1)
        else:
            raise RuntimeError("Disposable PostgreSQL did not become ready")
        sql("""
CREATE ROLE webdock_platform_owner;
CREATE ROLE webdock_auth_runtime;
CREATE ROLE webdock_auth_bootstrap;
CREATE SCHEMA webdock_auth AUTHORIZATION webdock_platform_owner;
CREATE SCHEMA webdock_rls;
CREATE SCHEMA webdock_live;
GRANT USAGE ON SCHEMA webdock_auth TO webdock_auth_runtime,webdock_auth_bootstrap;
GRANT USAGE ON SCHEMA webdock_rls,webdock_live TO webdock_platform_owner;
CREATE FUNCTION webdock_rls.operator() RETURNS boolean LANGUAGE sql AS 'SELECT false';
CREATE FUNCTION webdock_live.invalidate_directory() RETURNS trigger LANGUAGE plpgsql AS $$BEGIN RETURN NULL; END$$;
CREATE TABLE webdock_auth."rateLimit"(id text PRIMARY KEY,key text NOT NULL UNIQUE,count integer NOT NULL,"lastRequest" bigint NOT NULL);
ALTER TABLE webdock_auth."rateLimit" OWNER TO webdock_platform_owner;
ALTER TABLE webdock_auth."rateLimit" ENABLE ROW LEVEL SECURITY;
ALTER TABLE webdock_auth."rateLimit" FORCE ROW LEVEL SECURITY;
CREATE POLICY platform_owner ON webdock_auth."rateLimit" TO webdock_platform_owner USING(true) WITH CHECK(true);
""")
        sql((MIGRATIONS / "0068_waitlist.sql").read_text())
        sql((MIGRATIONS / "0073_waitlist_notice_version.sql").read_text())
        assert submit("1", "existing@example.test")
        sql("UPDATE webdock_auth.waitlist_entry SET status='rejected',reviewed_by='operator',reviewed_at=now(),version=2;")
        # Forward upgrades preserve review decisions and previously charged contacts.
        sql((MIGRATIONS / "0076_waitlist_contact_budget.sql").read_text())
        sql((MIGRATIONS / "0081_waitlist_budget_initialization.sql").read_text())
        sql("""SET ROLE webdock_auth_bootstrap;
SELECT webdock_auth.submit_waitlist((n+10)::text,'existing@example.test',NULL) FROM generate_series(1,1005)n;""")
        assert counts() == [1, 1], "same-email submissions consumed the global allowance"
        assert sql("SELECT status||':'||version FROM webdock_auth.waitlist_entry WHERE id='1'") == "rejected:2"
        assert submit("1100", "new@example.test"), "another contact was blocked by repeated email attempts"
        assert counts() == [2, 2]
        assert sql("SELECT count FROM webdock_auth.\"rateLimit\" WHERE key LIKE 'waitlist:email:%' ORDER BY count DESC LIMIT 1") == "6"
        assert sql("SELECT proowner='webdock_platform_owner'::regrole AND prosecdef FROM pg_proc WHERE oid='webdock_auth.submit_waitlist(text,text,text)'::regprocedure") == "t"
        assert sql("SELECT has_function_privilege('webdock_auth_bootstrap','webdock_auth.submit_waitlist(text,text,text)','EXECUTE') AND NOT has_function_privilege('webdock_auth_runtime','webdock_auth.submit_waitlist(text,text,text)','EXECUTE')") == "t"
        reset()
        sql("""INSERT INTO webdock_auth."rateLimit"(id,key,count,"lastRequest")
SELECT 'waitlist:email:'||encode(sha256(convert_to('throttled@example.test','UTF8')),'hex'),
       'waitlist:email:'||encode(sha256(convert_to('throttled@example.test','UTF8')),'hex'),
       5,floor(extract(epoch FROM statement_timestamp())*1000)::bigint;""")
        assert not submit("1900", "throttled@example.test")
        assert counts() == [0, 0], "a per-email rejection must not debit the new-contact allowance"
        reset()
        assert sum(concurrent(["same@example.test"] * 12, 2000)) == 5
        assert counts() == [1, 1], "concurrent duplicates must debit only one contact"
        # Fill 999 real new contacts, then race for the final slot in separate transactions.
        reset()
        assert sql("""SET ROLE webdock_auth_bootstrap;
SELECT bool_and(webdock_auth.submit_waitlist((n+3000)::text,'person-'||n||'@example.test',NULL))
FROM generate_series(1,999)n;""") == "t"
        assert counts() == [999, 999]
        assert sum(concurrent([f"last-{i}@example.test" for i in range(12)], 5000)) == 1
        assert counts() == [1000, 1000]
        assert sql('SELECT count(*) FROM webdock_auth."rateLimit"') == "1001", "rejected new emails grew limiter state"
        assert not submit("6000", "person-1@example.test"), "capacity reply must not reveal existing contacts"
        assert not submit("6001", "overflow@example.test")
        assert counts() == [1000, 1000]
        sql('UPDATE webdock_auth."rateLimit" SET "lastRequest"="lastRequest"-86400001 WHERE key=\'waitlist:global\';')
        assert submit("6002", "person-1@example.test")
        assert counts() == [1000, 0], "an existing contact must not charge the new daily window"
        assert submit("6003", "next-day@example.test")
        assert counts() == [1001, 1]
        print(json.dumps({"passed": True, "postgres": "18.6", "forwardMigrationPreservesReview": True,
                          "repeatedSubmissions": 1005, "concurrentDuplicateAttempts": 12,
                          "concurrentFinalSlotAttempts": 12, "dailyNewContactLimit": 1000,
                          "globalCapResponseIndependentOfExistingEmail": True,
                          "noProductionDatabase": True}))
    finally:
        if container:
            state = json.loads(docker("inspect", container))[0]
            assert state["Name"] == "/" + name and state["Config"]["Labels"]["spitzli.waitlist-fixture"] == name
            docker("rm", "-f", container)


if __name__ == "__main__":
    run()
