import { withAuthenticatedDatabase } from "../../../db/database-context";
import type {
  MailActor,
  MailCapability,
  Permission,
} from "@webdock/mail-contracts";
import type { DatabaseReader, SessionReference } from "@webdock/core/identity";
import { selectedIdentityDirectory as betterAuthIdentity } from "../../auth/principal";
import {
  createMailAuthorization,
  mailUnavailable,
} from "@webdock/module-mail/server/product-access";
import { resolveSessionReference } from "../../auth/principal";
import { mailDatabase as database } from "./store";
export {
  MailError,
  mailUnavailable,
} from "@webdock/module-mail/server/product-access";
const policy = createMailAuthorization({
  identity: betterAuthIdentity,
  byokAllowed: async (db, customerID) =>
    (await (await import("../../hosting/byok")).byokPolicy(db, customerID))
      .turbosmtp,
});
export function assertMailAccess(
  actor: MailActor,
  capability: MailCapability,
  db: DatabaseReader = database,
  admission = true,
): Promise<void> {
  return policy.assertMailAccess(actor, capability, db, admission);
}
export function assertMailManager(
  reference: SessionReference,
  customerID: string,
  db: DatabaseReader = database,
) {
  return policy.assertMailManager(reference, customerID, db);
}
export async function authorizeMailSession(
  headers: Headers,
  customerID: string,
  connectionID: string,
  capability: MailCapability,
): Promise<MailActor> {
  return withAuthenticatedDatabase(headers, async () => {
    const session = await resolveSessionReference(headers);
    if (!session) mailUnavailable();
    const actor: MailActor = {
      customerID,
      connectionID,
      principal: {
        kind: "session",
        userID: session.subject,
        sessionID: session.sessionID,
      },
    };
    await assertMailAccess(actor, capability);
    return actor;
  });
}
export async function authorizeMailKey(
  request: Request,
  permission: Permission,
): Promise<MailActor> {
  const { authorizeMailKey: authorize } = await import("./api-keys");
  return authorize(request, permission);
}
