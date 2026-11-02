/** Check before importing modules that own a pool, not only before fixture writes. */
export function assertMailTestDatabase(
  raw: string,
  env: Record<string, string | undefined> = process.env,
): void {
  const url = new URL(raw);
  if (
    !["postgresql:", "postgres:"].includes(url.protocol) ||
    !["localhost", "127.0.0.1"].includes(url.hostname) ||
    url.pathname !== "/webdock_api_test" ||
    url.search ||
    url.hash ||
    env.AUTH_TEST_MAIL !== "true" ||
    env.NODE_ENV === "production" ||
    env.VERCEL_ENV
  ) {
    throw Error("Disposable local Mail test database required");
  }
}

export function validEmail() {
  return {
    from: "sender@example.invalid",
    to: "delivered@test.mail.webdock.dev",
    subject: "Mail fixture",
    text: "Synthetic test content",
  };
}

export async function withMailFixture<T>(
  run: (fixture: Awaited<ReturnType<typeof createFixture>>) => Promise<T>,
): Promise<T> {
  assertMailTestDatabase(process.env.DATABASE_URL!);
  const fixture = await createFixture();
  try {
    return await run(fixture);
  } finally {
    await fixture.cleanup();
  }
}

async function createFixture() {
  assertMailTestDatabase(process.env.DATABASE_URL!);
  const { randomBytes } = await import("node:crypto");
  const { database: db } = await import("../../../src/db/runtime");
  const { auth } = await import("../../../src/modules/auth/auth");
  const { createIdentity } =
    await import("../../../src/modules/auth/bootstrap");
  const suffix = randomBytes(8).toString("hex");
  const users: string[] = [],
    customers: string[] = [],
    organizations: string[] = [];
  async function identity(label: string, operator = false) {
    const password = randomBytes(32).toString("base64url");
    const user = await createIdentity({
      name: "Mail fixture",
      email: `${label}-${suffix}@example.invalid`,
      password,
      operator,
      mustChangePassword: false,
    });
    users.push(user.id);
    const origin = process.env.BETTER_AUTH_URL!;
    const response = await auth.handler(
      new Request(origin + "/api/auth/sign-in/email", {
        method: "POST",
        headers: {
          Origin: origin,
          "Content-Type": "application/json",
          "x-webdock-client-ip": `192.0.2.${1 + (randomBytes(1)[0] % 250)}`,
        },
        body: JSON.stringify({ email: user.email, password }),
      }),
    );
    if (response.status !== 200) throw Error("Fixture sign-in failed");
    if (operator)
      await db.query(
        'UPDATE webdock_auth."user" SET "twoFactorEnabled"=true WHERE id=$1',
        [user.id],
      );
    const headers = new Headers({
      Origin: origin,
      Cookie: response.headers
        .getSetCookie()
        .map((c) => c.split(";")[0])
        .join("; "),
    });
    const session = await auth.api.getSession({ headers });
    if (!session) throw Error("Fixture session missing");
    return {
      id: user.id,
      headers,
      principal: {
        kind: "session" as const,
        userID: user.id,
        sessionID: session.session.id,
      },
    };
  }
  async function cleanup() {
    await db.query(
      "DELETE FROM webdock_auth.access_event WHERE actor_id=ANY($1)",
      [users],
    );
    await db.query(
      "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=ANY($1)",
      [customers],
    );
    await db.query('DELETE FROM webdock_auth.member WHERE "userId"=ANY($1)', [
      users,
    ]);
    await db.query("DELETE FROM webdock_admin.customers WHERE id=ANY($1)", [
      customers,
    ]);
    await db.query("DELETE FROM webdock_auth.organization WHERE id=ANY($1)", [
      organizations,
    ]);
    await db.query('DELETE FROM webdock_auth."user" WHERE id=ANY($1)', [users]);
  }
  try {
    const admin = await identity("admin"),
      other = await identity("other"),
      member = await identity("member"),
      operator = await identity("operator", true);
    for (const who of [admin, other]) {
      const c = (
        await db.query(
          "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),$1) RETURNING id",
          ["Mail fixture " + suffix],
        )
      ).rows[0];
      customers.push(c.id);
      const t = (
        await db.query(
          "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
          [c.id],
        )
      ).rows[0];
      organizations.push(t.organization_id);
      await db.query(
        'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'admin\',now())',
        [t.organization_id, who.id],
      );
    }
    await db.query(
      'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'member\',now())',
      [organizations[0], member.id],
    );
    return {
      db,
      suffix,
      customers,
      organizations,
      admin,
      other,
      member,
      operator,
      adminHeaders: admin.headers,
      memberHeaders: member.headers,
      operatorHeaders: operator.headers,
      now: new Date("2026-10-12T12:00:00Z"),
      setNow(instant: Date) {
        this.now = instant;
      },
      cleanup,
    };
  } catch (error) {
    await cleanup();
    throw error;
  }
}

export async function withMailProductFixture<T>(
  run: (
    fixture: Awaited<ReturnType<typeof createFixture>> & {
      actorA: import("@webdock/mail-contracts").MailActor;
      actorB: import("@webdock/mail-contracts").MailActor;
      domainA: { id: string; name: string };
    },
  ) => Promise<T>,
): Promise<T> {
  return withMailFixture(async (f) => {
    const { mailProductMigrations } =
      await import("../../../src/modules/mail/product/schema");
    for (const [, sql] of mailProductMigrations) await f.db.query(sql);
    const previous = (
      await f.db.query(
        "SELECT * FROM webdock_auth.platform_settings WHERE id=true",
      )
    ).rows[0];
    const connections: string[] = [];
    try {
      await f.db.query(
        "INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,$1) ON CONFLICT(id) DO UPDATE SET settings=$1",
        [
          JSON.stringify({
            ...previous?.settings,
            mailEnabled: true,
            mailGatewayEnabled: true,
            mailDispatchEnabled: true,
            mailWebhooksEnabled: true,
          }),
        ],
      );
      for (const c of f.customers)
        connections.push(
          (
            await f.db.query(
              "INSERT INTO webdock_auth.mail_connection(customer_id,kind,provider_account_id,state) VALUES($1,'platform',$2,'ready') RETURNING id",
              [c, "fixture-" + c],
            )
          ).rows[0].id,
        );
      const name = f.suffix + ".example.invalid";
      const domain = (
        await f.db.query(
          "INSERT INTO webdock_auth.mail_sender_domain(customer_id,domain,token,status) VALUES($1,$2,$3,'ownership_verified') RETURNING id",
          [f.customers[0], name, f.suffix],
        )
      ).rows[0];
      await f.db.query(
        "INSERT INTO webdock_auth.mail_connection_domain(customer_id,connection_id,domain_id,provider_id,provider_snapshot,provider_checked_at) VALUES($1,$2,$3,'fixture-domain',$4,now())",
        [
          f.customers[0],
          connections[0],
          domain.id,
          JSON.stringify({
            id: "fixture-domain",
            domain: name,
            spf_verified: true,
            dkim_verified: true,
            dmarc_verified: true,
          }),
        ],
      );
      await f.db.query(
        "UPDATE webdock_auth.mail_sender_domain SET provider_id=$2,provider_snapshot=$3,provider_checked_at=now() WHERE id=$1",
        [
          domain.id,
          "fixture-domain",
          JSON.stringify({
            id: "fixture-domain",
            domain: name,
            spf_verified: true,
            dkim_verified: true,
            dmarc_verified: true,
          }),
        ],
      );
      return await run(
        Object.assign(f, {
          actorA: {
            customerID: f.customers[0],
            connectionID: connections[0],
            principal: f.admin.principal,
          },
          actorB: {
            customerID: f.customers[1],
            connectionID: connections[1],
            principal: f.other.principal,
          },
          domainA: { id: domain.id, name },
        }),
      );
    } finally {
      for (const table of [
        "mail_connection_tracking_settings",
        "mail_tracking_domain",
        "mail_tracking_settings",
        "mail_tracking_operation",
        "mail_import_map",
        "mail_request_log",
        "mail_webhook_attempt",
        "mail_webhook_delivery",
        "mail_webhook",
        "mail_event",
        "mail_idempotency",
        "mail_batch_item",
        "mail_batch",
        "mail_usage_reservation",
        "mail_usage_period",
        "mail_message",
        "mail_suppression",
        "mail_rate_window",
        "mail_api_key",
        "mail_connection_domain",
        "mail_provider_operation",
        "mail_connection_secret",
        "mail_connection",
        "mail_sender_domain",
      ])
        await f.db.query(
          `DELETE FROM webdock_auth.${table} WHERE customer_id=ANY($1)`,
          [f.customers],
        );
      if (previous)
        await f.db.query(
          "UPDATE webdock_auth.platform_settings SET settings=$1 WHERE id=true",
          [previous.settings],
        );
      else
        await f.db.query(
          "DELETE FROM webdock_auth.platform_settings WHERE id=true",
        );
    }
  });
}
