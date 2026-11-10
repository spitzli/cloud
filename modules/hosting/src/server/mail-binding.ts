import type { HostingPorts } from "./ports";
import { isIP } from "node:net";
import ipaddr from "ipaddr.js";
import {
  HostingError,
  type HostingActor,
  type ServiceRuntime,
} from "@spitzli/hosting-contracts";
import type { MailActor } from "@spitzli/mail-contracts";
import type { Connection } from "./authorization";
import type { BindingMaterial } from "./binding-runtime";
export type Dependencies = {
  requireMail: HostingPorts["requireMail"];
  currentDatabaseScope: HostingPorts["currentDatabaseScope"];
  assertMailAccess: HostingPorts["assertMailAccess"];
  createMailKeyInTransaction: HostingPorts["createMailKeyInTransaction"];
  env: HostingPorts["env"];
};

export function createHostingMailBinding(dependencies: Dependencies) {
  function applicationMailRoute() {
    dependencies.requireMail();
    try {
      const origin = new URL(dependencies.env.MAIL_API_ORIGIN!);
      const egressCIDRs = (
        dependencies.env.SPITZLI_APPLICATION_MAIL_EGRESS_CIDRS ?? ""
      )
        .split(",")
        .map((v) => v.trim())
        .filter(Boolean);
      if (
        origin.protocol !== "https:" ||
        origin.username ||
        origin.password ||
        origin.port ||
        origin.pathname !== "/" ||
        origin.search ||
        origin.hash ||
        !egressCIDRs.length ||
        egressCIDRs.length > 8
      )
        throw Error();
      for (const cidr of egressCIDRs) {
        const [ip, bits, ...rest] = cidr.split("/");
        if (
          rest.length ||
          !(
            (isIP(ip) === 4 && bits === "32") ||
            (isIP(ip) === 6 && bits === "128")
          ) ||
          ["loopback", "unspecified", "multicast"].includes(
            ipaddr.process(ip).range(),
          )
        )
          throw Error();
      }
      return { origin: origin.origin, egressCIDRs: [...new Set(egressCIDRs)] };
    } catch {
      throw new HostingError(
        409,
        "Configure verified native Mail API egress before connecting applications.",
      );
    }
  }
  async function mailBindingMaterial(
    db: Connection,
    actor: HostingActor,
    input: {
      customerID: string;
      connectionID: string;
      domainID: string;
      bindingID: string;
      variable: string;
    },
    previous?: BindingMaterial,
  ): Promise<BindingMaterial> {
    dependencies.requireMail("access");
    if (!["studio", "oauth"].includes(actor.source))
      throw new HostingError(
        403,
        "Native Mail connection requires authorized management.",
      );
    const mailActor: MailActor = {
      customerID: input.customerID,
      connectionID: input.connectionID,
      principal: {
        kind: "session",
        userID: actor.subject,
        sessionID: actor.sessionID,
      },
    };
    await dependencies.assertMailAccess(mailActor, "manage", db);
    await dependencies.assertMailAccess(mailActor, "send", db);
    const domain = (
      await db.query(
        `SELECT b.provider_snapshot,b.provider_checked_at FROM webdock_auth.mail_connection_domain b JOIN webdock_auth.mail_sender_domain d ON d.id=b.domain_id AND d.customer_id=b.customer_id WHERE b.customer_id=$1 AND b.connection_id=$2 AND d.id=$3 AND d.deleted_at IS NULL AND d.status='ownership_verified' AND d.provider_operation IS NULL`,
        [input.customerID, input.connectionID, input.domainID],
      )
    ).rows[0];
    if (
      !domain?.provider_checked_at ||
      Date.now() - new Date(domain.provider_checked_at).getTime() > 300000 ||
      domain.provider_snapshot?.spf_verified !== true ||
      domain.provider_snapshot?.dkim_verified !== true
    )
      throw new HostingError(
        409,
        "Verify the sender domain before connecting native Mail.",
      );
    if (input.variable === "SPITZLI_MAIL_API_URL")
      throw new HostingError(
        409,
        "Choose a separate variable name for the Mail key.",
      );
    const route = applicationMailRoute();
    const previousKey = previous?.mailKeyID
      ? (
          await db.query(
            "SELECT id,revoked_at,domain_id FROM webdock_auth.mail_api_key WHERE id=$1 AND customer_id=$2 AND connection_id=$3",
            [previous.mailKeyID, input.customerID, input.connectionID],
          )
        ).rows[0]
      : null;
    let key =
      previousKey &&
      !previousKey.revoked_at &&
      previousKey.domain_id === input.domainID
        ? previous!.environment[input.variable]
        : undefined;
    let keyID = previousKey?.id;
    if (!key) {
      const created = await dependencies.createMailKeyInTransaction(
        db,
        mailActor,
        {
          name: "Application " + input.bindingID,
          permission: "sending_access",
          domainID: input.domainID,
        },
      );
      key = created.key;
      keyID = created.id;
    }
    const runtime: ServiceRuntime = {
      id: input.bindingID,
      revision: 1,
      kind: "mail",
      egressCIDRs: route.egressCIDRs,
    };
    return {
      environment: {
        [input.variable]: key,
        SPITZLI_MAIL_API_URL: route.origin,
      },
      runtime,
      credentialRevision: previous?.credentialRevision ?? 1,
      mailKeyID: keyID,
    };
  }
  async function revokeBindingMail(
    db: Connection,
    bindingID: string,
    retiringOnly = false,
  ) {
    if (
      dependencies.env.DATABASE_RLS_SECRET &&
      dependencies.currentDatabaseScope()?.kind === "system"
    ) {
      await db.query("SELECT webdock_rls.revoke_binding_mail_key($1,$2)", [
        bindingID,
        retiringOnly,
      ]);
      return;
    }
    await db.query(
      `UPDATE webdock_auth.mail_api_key k SET revoked_at=coalesce(k.revoked_at,now()) FROM webdock_auth.hosting_service_binding b WHERE b.id=$1 AND k.customer_id=b.customer_id AND (k.id=b.retiring_mail_key_id OR (NOT $2 AND k.id=b.mail_key_id))`,
      [bindingID, retiringOnly],
    );
    await db.query(
      "UPDATE webdock_auth.hosting_service_binding SET retiring_mail_key_id=NULL WHERE id=$1",
      [bindingID],
    );
  }
  async function bindingMailReady(
    db: Connection,
    bindingID: string,
  ): Promise<boolean> {
    if (
      dependencies.env.DATABASE_RLS_SECRET &&
      dependencies.currentDatabaseScope()?.kind === "system"
    )
      return (
        (
          await db.query("SELECT webdock_rls.binding_mail_ready($1) AS ready", [
            bindingID,
          ])
        ).rows[0]?.ready === true
      );
    return Boolean(
      (
        await db.query(
          `SELECT 1 FROM webdock_auth.hosting_service_binding b JOIN webdock_auth.mail_api_key k ON k.id=b.mail_key_id AND k.customer_id=b.customer_id JOIN webdock_auth.mail_connection c ON c.id=k.connection_id AND c.customer_id=b.customer_id JOIN webdock_auth.mail_sender_domain d ON d.id=k.domain_id AND d.customer_id=b.customer_id CROSS JOIN webdock_auth.platform_settings p WHERE b.id=$1 AND k.revoked_at IS NULL AND c.state='ready' AND c.deleted_at IS NULL AND d.status='ownership_verified' AND d.deleted_at IS NULL AND p.id=true AND p.settings->>'mailGatewayEnabled'='true' AND (c.kind<>'platform' OR p.settings->>'mailEnabled'='true')`,
          [bindingID],
        )
      ).rowCount,
    );
  }
  return {
    applicationMailRoute,
    mailBindingMaterial,
    revokeBindingMail,
    bindingMailReady,
  };
}
