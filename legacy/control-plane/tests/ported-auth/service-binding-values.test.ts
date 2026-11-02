import test from "node:test";
import assert from "node:assert/strict";
import { databaseConnectionURL } from "../../src/modules/hosting/database-binding";
test("database binding escapes credentials and requires a verified mounted CA", () => {
  const value = databaseConnectionURL({
    host: "postgres-rw.wd-db-42.svc.cluster.local",
    port: 5432,
    database: "db_7",
    username: "wd_app_7",
    password: "a@b:/?#%",
    caPath: "/var/run/webdock/services/42/ca.crt",
  });
  const parsed = new URL(value);
  assert.equal(decodeURIComponent(parsed.password), "a@b:/?#%");
  assert.equal(parsed.hostname, "postgres-rw.wd-db-42.svc.cluster.local");
  assert.equal(parsed.searchParams.get("sslmode"), "verify-full");
  assert.equal(
    parsed.searchParams.get("sslrootcert"),
    "/var/run/webdock/services/42/ca.crt",
  );
  assert.throws(() =>
    databaseConnectionURL({
      host: "evil/path",
      port: 5432,
      database: "db_7",
      username: "wd_app_7",
      password: "secret",
      caPath: "/tmp/ca",
    }),
  );
});

test("native Mail routes reject destinations the hosting agent cannot apply", async () => {
  const { applicationMailRoute } =
    await import("../../src/modules/hosting/mail-binding");
  const origin = process.env.MAIL_API_ORIGIN,
    cidrs = process.env.WEBDOCK_APPLICATION_MAIL_EGRESS_CIDRS;
  try {
    process.env.MAIL_API_ORIGIN = "https://mail.example.test";
    for (const value of [
      "127.2.3.4/32",
      "224.0.0.1/32",
      "ff02::1/128",
      "0:0:0:0:0:0:0:1/128",
      "0:0:0:0:0:0:0:0/128",
    ]) {
      process.env.WEBDOCK_APPLICATION_MAIL_EGRESS_CIDRS = value;
      assert.throws(
        () => applicationMailRoute(),
        /verified native Mail API egress/,
        value,
      );
    }
    process.env.WEBDOCK_APPLICATION_MAIL_EGRESS_CIDRS =
      "10.96.0.12/32,2001:db8::1/128";
    assert.equal(applicationMailRoute().egressCIDRs.length, 2);
  } finally {
    if (origin === undefined) delete process.env.MAIL_API_ORIGIN;
    else process.env.MAIL_API_ORIGIN = origin;
    if (cidrs === undefined)
      delete process.env.WEBDOCK_APPLICATION_MAIL_EGRESS_CIDRS;
    else process.env.WEBDOCK_APPLICATION_MAIL_EGRESS_CIDRS = cidrs;
  }
});
