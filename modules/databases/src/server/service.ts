type StoredConsoleIdentity = NonNullable<
  import("@spitzli/hosting-contracts").HostingActor["consoleIdentity"]
>;
import { createHash, randomBytes } from "node:crypto";
import {
  databaseCommand,
  publicBinding,
  runtimeOrigin,
  type GatewayAccess,
} from "@spitzli/database-contracts";
import { HostingError, type HostingActor } from "@spitzli/hosting-contracts";
import type { DatabaseReader as Connection } from "@spitzli/core/identity";

export interface DatabaseServiceDependencies {
  /** Provider authority validates grants; the local user row supplies only identity and bans. */
  recipientAuthority?: {
    isExternal(): boolean;
    allowed(subject: string, customerID: string): Promise<boolean>;
  };
  storedConsoleIdentity(): StoredConsoleIdentity | null;
  withStoredIdentity<T>(
    reference: StoredConsoleIdentity,
    run: (principal: { userID: string; sessionID: string }) => Promise<T>,
  ): Promise<T>;

  authorizeHosting(
    actor: HostingActor,
    target: {
      customerID?: string;
      projectID?: string;
      write?: boolean;
      operator?: boolean;
      live?: boolean;
      tenantAdmin?: boolean;
      permission?: string;
    },
    db: Connection,
  ): Promise<{ operator: boolean; canWrite: boolean }>;
  hostingAudit(
    db: Connection,
    actor: HostingActor,
    action: string,
    target: string,
  ): Promise<unknown>;
  encryptMailSecret(value: unknown, purpose: string): string;
  decryptMailSecret<T>(value: string, purpose: string): T;
  gatewayOrigin(): string | undefined;
}
/** API composition supplies local authorization, encryption and runtime configuration; all database fences stay here. */
export function createDatabaseService(
  dependencies: DatabaseServiceDependencies,
) {
  const {
    storedConsoleIdentity,
    withStoredIdentity,
    authorizeHosting,
    hostingAudit,
    encryptMailSecret,
    decryptMailSecret,
  } = dependencies;
  const fail = () => new HostingError(404, "Database access is unavailable.");
  const hash = (token: string) =>
    createHash("sha256").update(token).digest("hex");
  const token = () => randomBytes(32).toString("base64url");
  const selection = `SELECT b.id,b.project_id AS "projectID",p.customer_id AS "customerID",b.name,b.environment,b.engine,g.profile
 FROM webdock_auth.database_binding b JOIN webdock_admin.projects p ON p.id=b.project_id
 LEFT JOIN webdock_auth.database_grant g ON g.binding_id=b.id AND g.subject=$1 AND g.enabled
 WHERE b.enabled AND p.status='active'`;

  async function binding(
    db: Connection,
    actor: HostingActor,
    id: string,
    operator = false,
  ) {
    if (actor.source !== "studio") throw fail();
    const row = (
      await db.query(selection + " AND b.id=$2", [actor.subject, id])
    ).rows[0];
    if (!row) throw fail();
    const authorization = await authorizeHosting(
      actor,
      {
        permission: "org:databases:manage",
        projectID: row.projectID,
        live: true,
        operator,
      },
      db,
    );
    if (!operator && !row.profile) throw fail();
    return { row, authorization };
  }

  async function audit(
    db: Connection,
    actor: HostingActor,
    action: string,
    bindingID: string,
  ) {
    await db.query(
      "INSERT INTO webdock_auth.database_audit(subject,action,binding_id) VALUES($1,$2,$3)",
      [actor.subject, action, bindingID],
    );
  }

  async function executeDatabase(
    db: Connection,
    actor: HostingActor,
    input: unknown,
  ): Promise<unknown> {
    if (actor.source !== "studio") throw fail();
    const cmd = databaseCommand.parse(input);
    if (cmd.action === "list") {
      const auth = await authorizeHosting(
        actor,
        {
          permission: "org:databases:manage",
          customerID: cmd.customerID,
          live: true,
        },
        db,
      );
      const rows = (
        await db.query(
          selection +
            " AND p.customer_id=$2 AND ($3::boolean OR g.id IS NOT NULL) ORDER BY b.name,b.id LIMIT 200",
          [actor.subject, cmd.customerID, auth.operator],
        )
      ).rows;
      const projects = auth.operator
        ? (
            await db.query(
              "SELECT id,name FROM webdock_admin.projects WHERE customer_id=$1 AND status='active' ORDER BY name LIMIT 200",
              [cmd.customerID],
            )
          ).rows
        : [];
      const people = auth.operator
        ? (
            await db.query(
              dependencies.recipientAuthority?.isExternal()
                ? `SELECT u.id,u.name,u.email FROM webdock_auth."user" u WHERE NOT COALESCE(u.banned,false) AND $1::text IS NOT NULL ORDER BY u.name LIMIT 200`
                : `SELECT u.id,u.name,u.email FROM webdock_auth."user" u WHERE NOT COALESCE(u.banned,false) AND
      (u.role='operator' OR EXISTS(SELECT 1 FROM webdock_auth.member m JOIN webdock_auth.tenant_customer t ON t.organization_id=m."organizationId"
       WHERE m."userId"=u.id AND t.customer_id=$1)) ORDER BY u.name LIMIT 200`,
              [cmd.customerID],
            )
          ).rows
        : [];
      return {
        managed: (
          await db.query(
            `SELECT d.id,d.project_id AS "projectID",d.name,d.state,
         ($2::boolean AND d.state='ready' AND d.pending_backend_id IS NULL
          AND d.confirmed_credentials_encrypted IS NOT NULL AND d.confirmed_credential_revision=d.credential_revision
          AND b.state='ready' AND b.endpoint_ready AND COALESCE(b.ca_certificate,'')<>'') AS "canOpen"
         FROM webdock_auth.hosting_database d JOIN webdock_admin.projects p ON p.id=d.project_id
         JOIN webdock_auth.hosting_database_backend b ON b.id=d.backend_id
         WHERE d.customer_id=$1 AND p.status='active' AND d.state<>'purged' ORDER BY d.name,d.id LIMIT 200`,
            [
              cmd.customerID,
              auth.canWrite && actor.scopes.includes("hosting:write"),
            ],
          )
        ).rows,
        bindings: rows.map((row) => publicBinding.parse(row)),
        operator: auth.operator,
        projects,
        people,
      };
    }
    if (cmd.action === "create") {
      await authorizeHosting(
        actor,
        {
          permission: "org:databases:manage",
          projectID: cmd.projectID,
          operator: true,
          write: true,
        },
        db,
      );
      const row = (
        await db.query(
          "INSERT INTO webdock_auth.database_binding(project_id,name,environment,engine) VALUES($1,$2,$3,$4) RETURNING id",
          [cmd.projectID, cmd.name, cmd.environment, cmd.engine],
        )
      ).rows[0];
      await audit(db, actor, "binding.create", row.id);
      return { id: row.id };
    }
    if (cmd.action === "openManaged")
      return openManagedDatabase(db, actor, cmd.databaseID);
    const management = ["grant", "revoke", "disable"].includes(cmd.action);
    const { row } = await binding(db, actor, cmd.bindingID, management);
    if (cmd.action === "get") return publicBinding.parse(row);
    if (cmd.action === "grant") {
      const external = dependencies.recipientAuthority?.isExternal() === true;
      if (external && !await dependencies.recipientAuthority!.allowed(cmd.subject, row.customerID)) throw fail();
      const user = (
        await db.query(
          external
            ? `SELECT u.id FROM webdock_auth."user" u WHERE u.id=$1 AND NOT COALESCE(u.banned,false) AND $2::text IS NOT NULL`
            : `SELECT u.id FROM webdock_auth."user" u
      WHERE u.id=$1 AND NOT COALESCE(u.banned,false) AND (u.role='operator' OR EXISTS(
       SELECT 1 FROM webdock_auth.member m JOIN webdock_auth.tenant_customer t ON t.organization_id=m."organizationId"
       WHERE m."userId"=u.id AND t.customer_id=$2))`,
          [cmd.subject, row.customerID],
        )
      ).rows[0];
      if (!user) throw fail();
      const old = (
        await db.query(
          "SELECT profile,runtime_origin FROM webdock_auth.database_grant WHERE binding_id=$1 AND subject=$2 FOR UPDATE",
          [cmd.bindingID, cmd.subject],
        )
      ).rows[0];
      if (
        old &&
        old.profile !== cmd.profile &&
        old.runtime_origin === cmd.runtimeOrigin
      ) {
        throw new HostingError(
          409,
          "Changing database privileges requires a newly isolated runtime.",
        );
      }
      const occupied = (
        await db.query(
          "SELECT id FROM webdock_auth.database_grant WHERE runtime_origin=$1 AND NOT(binding_id=$2 AND subject=$3)",
          [cmd.runtimeOrigin, cmd.bindingID, cmd.subject],
        )
      ).rows[0];
      if (occupied)
        throw new HostingError(
          409,
          "This runtime is already assigned to another database user.",
        );
      const claimed = (
        await db.query(
          `INSERT INTO webdock_auth.database_runtime_owner(runtime_origin,binding_id,subject,profile) VALUES($1,$2,$3,$4)
      ON CONFLICT(runtime_origin) DO UPDATE SET runtime_origin=EXCLUDED.runtime_origin
      WHERE database_runtime_owner.binding_id=EXCLUDED.binding_id AND database_runtime_owner.subject=EXCLUDED.subject AND database_runtime_owner.profile=EXCLUDED.profile
      RETURNING runtime_origin`,
          [cmd.runtimeOrigin, cmd.bindingID, cmd.subject, cmd.profile],
        )
      ).rows[0];
      if (!claimed)
        throw new HostingError(
          409,
          "This runtime is already assigned to another database user.",
        );
      const secret = encryptMailSecret(
        cmd.proxySecret,
        `database:${cmd.bindingID}:${cmd.subject}`,
      );
      await db.query(
        `INSERT INTO webdock_auth.database_grant(binding_id,subject,profile,runtime_origin,connection_id,secret_sealed)
      VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(binding_id,subject) DO UPDATE SET profile=EXCLUDED.profile,
      runtime_origin=EXCLUDED.runtime_origin,connection_id=EXCLUDED.connection_id,secret_sealed=EXCLUDED.secret_sealed,
      revision=database_grant.revision+1,enabled=true`,
        [
          cmd.bindingID,
          cmd.subject,
          cmd.profile,
          cmd.runtimeOrigin,
          cmd.connectionID,
          secret,
        ],
      );
      await audit(db, actor, "grant.configure", cmd.bindingID);
      return { ok: true };
    }
    if (cmd.action === "revoke") {
      await db.query(
        "UPDATE webdock_auth.database_grant SET enabled=false,revision=revision+1 WHERE binding_id=$1 AND subject=$2",
        [cmd.bindingID, cmd.subject],
      );
    } else if (cmd.action === "disable") {
      await db.query(
        "UPDATE webdock_auth.database_binding SET enabled=false WHERE id=$1",
        [cmd.bindingID],
      );
      await db.query(
        "UPDATE webdock_auth.database_grant SET enabled=false,revision=revision+1 WHERE binding_id=$1",
        [cmd.bindingID],
      );
    } else if (cmd.action === "open") {
      const origin = runtimeOrigin.safeParse(dependencies.gatewayOrigin());
      if (!origin.success)
        throw new HostingError(503, "Database gateway is not configured.");
      const code = token();
      const launch = (
        await db.query(
          `INSERT INTO webdock_auth.database_launch(token_hash,grant_id,grant_revision,session_id,expires_at,clerk_identity)
        SELECT $1,id,revision,$2,now()+interval '60 seconds',$5::jsonb FROM webdock_auth.database_grant
      WHERE binding_id=$3 AND subject=$4 AND enabled RETURNING expires_at`,
          [
            hash(code),
            storedConsoleIdentity() ? null : actor.sessionID,
            cmd.bindingID,
            actor.subject,
            storedConsoleIdentity(),
          ],
        )
      ).rows[0];
      if (!launch) throw fail();
      await audit(db, actor, "session.launch", cmd.bindingID);
      return {
        code,
        gatewayOrigin: origin.data,
        binding: publicBinding.parse(row),
        expiresAt: new Date(launch.expires_at).toISOString(),
      };
    }
    await audit(db, actor, cmd.action, cmd.bindingID);
    return { ok: true };
  }

  async function authorizeGrant(
    db: Connection,
    grantID: string,
    revision: number,
    sessionID: string,
    reference?: StoredConsoleIdentity | null,
  ) {
    if (reference) {
      return withStoredIdentity(reference, async (principal) => {
        const grant = (
          await db.query(
            `SELECT g.*,b.project_id FROM webdock_auth.database_grant g JOIN webdock_auth.database_binding b ON b.id=g.binding_id WHERE g.id=$1 AND g.revision=$2 AND g.enabled AND b.enabled AND g.subject=$3 FOR SHARE OF g,b`,
            [grantID, revision, principal.userID],
          )
        ).rows[0];
        if (!grant) throw fail();
        await authorizeHosting(
          {
            subject: principal.userID,
            sessionID: principal.sessionID,
            source: "studio",
            scopes: ["hosting:read"],
          },
          {
            permission: "org:databases:manage",
            projectID: grant.project_id,
            live: true,
          },
          db,
        );
        return grant;
      });
    }
    const grant = (
      await db.query(
        `SELECT g.*,b.project_id FROM webdock_auth.database_grant g
    JOIN webdock_auth.database_binding b ON b.id=g.binding_id JOIN webdock_auth.session s ON s.id=$3 AND s."userId"=g.subject
    WHERE g.id=$1 AND g.revision=$2 AND g.enabled AND b.enabled FOR SHARE OF g,b,s`,
        [grantID, revision, sessionID],
      )
    ).rows[0];
    if (!grant) throw fail();
    await authorizeHosting(
      {
        subject: grant.subject,
        sessionID,
        source: "studio",
        scopes: ["hosting:read"],
      },
      {
        permission: "org:databases:manage",
        projectID: grant.project_id,
        live: true,
      },
      db,
    );
    return grant;
  }

  async function exchangeLaunch(db: Connection, code: string) {
    if (!/^[A-Za-z0-9_-]{43}$/.test(code)) throw fail();
    const launch = (
      await db.query(
        `UPDATE webdock_auth.database_launch SET consumed_at=now()
    WHERE token_hash=$1 AND consumed_at IS NULL AND expires_at>now() RETURNING *`,
        [hash(code)],
      )
    ).rows[0];
    if (!launch) return exchangeManagedLaunch(db, code);
    await authorizeGrant(
      db,
      launch.grant_id,
      launch.grant_revision,
      launch.session_id,
      launch.clerk_identity,
    );
    const sessionToken = token();
    const session = (
      await db.query(
        `INSERT INTO webdock_auth.database_session(token_hash,grant_id,grant_revision,session_id,expires_at,clerk_identity)
      VALUES($1,$2,$3,$4,now()+interval '15 minutes',$5::jsonb) RETURNING id,expires_at`,
        [
          hash(sessionToken),
          launch.grant_id,
          launch.grant_revision,
          launch.session_id,
          launch.clerk_identity,
        ],
      )
    ).rows[0];
    return {
      token: sessionToken,
      scopeID: session.id,
      expiresAt: new Date(session.expires_at).toISOString(),
    };
  }

  async function inspectSession(
    db: Connection,
    sessionToken: string,
  ): Promise<GatewayAccess> {
    if (!/^[A-Za-z0-9_-]{43}$/.test(sessionToken)) throw fail();
    const session = (
      await db.query(
        "SELECT * FROM webdock_auth.database_session WHERE token_hash=$1 AND expires_at>now()",
        [hash(sessionToken)],
      )
    ).rows[0];
    if (!session) return inspectManagedSession(db, sessionToken);
    const grant = await authorizeGrant(
      db,
      session.grant_id,
      session.grant_revision,
      session.session_id,
      session.clerk_identity,
    );
    return {
      scopeID: session.id,
      subject: grant.subject,
      connectionID: grant.connection_id,
      profile: grant.profile,
      runtimeOrigin: runtimeOrigin.parse(grant.runtime_origin),
      proxySecret: decryptMailSecret<string>(
        grant.secret_sealed,
        `database:${grant.binding_id}:${grant.subject}`,
      ),
      expiresAt: new Date(session.expires_at).toISOString(),
    };
  }

  const managedSelection = `SELECT d.*,b.host,b.direct_port,b.endpoint_ready,b.ca_certificate,b.state AS backend_state
 FROM webdock_auth.hosting_database d JOIN webdock_auth.hosting_database_backend b ON b.id=d.backend_id`;

  async function authorizeManaged(
    db: Connection,
    actor: HostingActor,
    databaseID: string,
  ) {
    if (actor.source !== "studio") throw fail();
    const d = (
      await db.query(managedSelection + " WHERE d.id=$1 FOR SHARE OF d,b", [
        databaseID,
      ])
    ).rows[0];
    if (!d) throw fail();
    await authorizeHosting(
      actor,
      {
        permission: "org:databases:manage",
        projectID: d.project_id,
        write: true,
        tenantAdmin: true,
        live: true,
      },
      db,
    );
    if (
      d.state !== "ready" ||
      d.pending_backend_id ||
      d.backend_state !== "ready" ||
      !d.endpoint_ready ||
      !d.ca_certificate ||
      !d.confirmed_credentials_encrypted ||
      d.confirmed_credential_revision !== d.credential_revision
    )
      throw fail();
    return d;
  }

  async function openManagedDatabase(
    db: Connection,
    actor: HostingActor,
    databaseID: string,
  ) {
    const d = await authorizeManaged(db, actor, databaseID);
    const origin = runtimeOrigin.safeParse(dependencies.gatewayOrigin());
    if (!origin.success)
      throw new HostingError(503, "Database gateway is not configured.");
    const code = token();
    const launch = (
      await db.query(
        `INSERT INTO webdock_auth.managed_database_launch(token_hash,database_id,backend_id,database_revision,credential_revision,subject,session_id,expires_at,clerk_identity)
       VALUES($1,$2,$3,$4,$5,$6,$7,now()+interval '60 seconds',$8::jsonb) RETURNING expires_at`,
        [
          hash(code),
          d.id,
          d.backend_id,
          d.revision,
          d.confirmed_credential_revision,
          actor.subject,
          storedConsoleIdentity() ? null : actor.sessionID,
          storedConsoleIdentity(),
        ],
      )
    ).rows[0];
    await hostingAudit(db, actor, "databases.viewer.launch", d.id);
    return {
      code,
      gatewayOrigin: origin.data,
      binding: publicBinding.parse({
        id: d.id,
        customerID: d.customer_id,
        projectID: d.project_id,
        name: d.name,
        environment: "production",
        engine: "postgresql",
        profile: "schema",
      }),
      expiresAt: new Date(launch.expires_at).toISOString(),
    };
  }

  async function authorizeManagedSession(
    db: Connection,
    session: {
      subject: string;
      session_id: string;
      clerk_identity?: StoredConsoleIdentity | null;
      database_id: string;
      backend_id: string;
      database_revision: number;
      credential_revision: number;
    },
  ): Promise<Awaited<ReturnType<typeof authorizeManaged>>> {
    if (session.clerk_identity) {
      return withStoredIdentity(session.clerk_identity, async (principal) => {
        if (principal.userID !== session.subject) throw fail();
        return authorizeManagedSession(db, {
          ...session,
          session_id: principal.sessionID,
          clerk_identity: null,
        });
      });
    }
    const d = await authorizeManaged(
      db,
      {
        subject: session.subject,
        sessionID: session.session_id,
        source: "studio",
        scopes: ["hosting:read", "hosting:write"],
      },
      session.database_id,
    );
    if (
      d.backend_id !== session.backend_id ||
      d.revision !== session.database_revision ||
      d.confirmed_credential_revision !== session.credential_revision
    )
      throw fail();
    return d;
  }

  async function exchangeManagedLaunch(db: Connection, code: string) {
    const launch = (
      await db.query(
        `UPDATE webdock_auth.managed_database_launch SET consumed_at=now()
     WHERE token_hash=$1 AND consumed_at IS NULL AND expires_at>now() RETURNING *`,
        [hash(code)],
      )
    ).rows[0];
    if (!launch) throw fail();
    await authorizeManagedSession(db, launch);
    const sessionToken = token();
    const session = (
      await db.query(
        `INSERT INTO webdock_auth.managed_database_session(token_hash,database_id,backend_id,database_revision,credential_revision,subject,session_id,expires_at,clerk_identity)
       VALUES($1,$2,$3,$4,$5,$6,$7,now()+interval '15 minutes',$8::jsonb) RETURNING id,expires_at`,
        [
          hash(sessionToken),
          launch.database_id,
          launch.backend_id,
          launch.database_revision,
          launch.credential_revision,
          launch.subject,
          launch.session_id,
          launch.clerk_identity,
        ],
      )
    ).rows[0];
    return {
      token: sessionToken,
      scopeID: session.id,
      expiresAt: new Date(session.expires_at).toISOString(),
    };
  }

  async function inspectManagedSession(
    db: Connection,
    sessionToken: string,
  ): Promise<GatewayAccess> {
    const session = (
      await db.query(
        "SELECT * FROM webdock_auth.managed_database_session WHERE token_hash=$1 AND expires_at>now()",
        [hash(sessionToken)],
      )
    ).rows[0];
    if (!session) throw fail();
    const d = await authorizeManagedSession(db, session);
    const secret = decryptMailSecret<{ migrationPassword: string }>(
      d.confirmed_credentials_encrypted,
      `database:${d.id}:${d.confirmed_credential_revision}`,
    );
    return {
      kind: "managed",
      scopeID: session.id,
      subject: session.subject,
      connectionID: d.id,
      profile: "schema",
      expiresAt: new Date(session.expires_at).toISOString(),
      revision: `${d.backend_id}:${d.confirmed_credential_revision}:${d.revision}`,
      database: {
        host: d.host,
        port: d.direct_port,
        database: d.database_name,
        username: `wd_migrate_${d.id}`,
        password: secret.migrationPassword,
        caCertificate: d.ca_certificate,
      },
    };
  }

  return { executeDatabase, exchangeLaunch, inspectSession };
}
