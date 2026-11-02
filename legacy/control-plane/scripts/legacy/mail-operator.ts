import { readFileSync } from "node:fs";
import {
  authorizeMailSession,
  assertMailAccess,
  MailError,
} from "../../src/modules/mail/product/access";
import { getMailHealth } from "../../src/modules/mail/product/health";
import { reconcileMail } from "../../src/modules/mail/product/reconcile";
import { dispatchMailStudio } from "../../src/modules/mail/product/studio";
import { database } from "../../src/db/runtime";
const [action, customerID, connectionID, sessionFile, resource] =
  process.argv.slice(2);
try {
  if (
    !["health", "pause", "resume", "reconcile"].includes(action) ||
    !customerID ||
    !connectionID ||
    !sessionFile
  )
    throw Error("usage");
  const cookie = readFileSync(sessionFile, "utf8").trim();
  if (cookie.length > 16384 || /[\r\n]/.test(cookie))
    throw Error("session_file");
  const headers = new Headers({ Cookie: cookie });
  const actor = await authorizeMailSession(
    headers,
    customerID,
    connectionID,
    "read",
  );
  await assertMailAccess(actor, "operate", database, false);
  if (action === "health")
    console.log(JSON.stringify(await getMailHealth(actor)));
  else if (action === "reconcile") {
    if (!resource) throw Error("message_id_required");
    await reconcileMail(actor, resource);
    console.log(
      JSON.stringify({
        action,
        customerID,
        connectionID,
        messageID: resource,
        status: "completed",
      }),
    );
  } else {
    if (!/^\d+$/.test(resource ?? "")) throw Error("current_revision_required");
    const result = await dispatchMailStudio(
      "mailConnectionUpdate",
      [customerID, connectionID, { action, revision: Number(resource) }],
      headers,
    );
    console.log(JSON.stringify({ action, customerID, connectionID, result }));
  }
} catch (error) {
  console.error(
    JSON.stringify({
      error: error instanceof MailError ? error.code : "operator_action_failed",
      usage:
        "mail-operator.ts health|pause|resume|reconcile CUSTOMER CONNECTION SESSION_COOKIE_FILE [REVISION|MESSAGE_ID]",
    }),
  );
  process.exitCode = 1;
} finally {
  await database.end();
}
