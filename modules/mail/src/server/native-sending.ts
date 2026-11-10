import { mailUnavailable } from "./native-access";
import type { createNativeMailboxes } from "./native-mailboxes";
import type { createNativeMailAuthorization } from "./native-access";
import type { createNativeKeys } from "./native-keys";
import type { createNativeSendTransport } from "./native-send-transport";
import type { NativeMailPorts } from "./native-ports";

export interface NativeSendingDependencies {
  resolveSessionReference: NativeMailPorts["resolveSessionReference"];
  withAuthenticatedDatabase: NativeMailPorts["withAuthenticatedDatabase"];
  queueMailConfiguration: ReturnType<
    typeof createNativeMailboxes
  >["queueMailConfiguration"];
  byokPolicy: NativeMailPorts["byokPolicy"];
  transaction: NativeMailPorts["transaction"];
  authorizeMail: ReturnType<
    typeof createNativeMailAuthorization
  >["authorizeMail"];
  manageNativeMailKey: ReturnType<
    typeof createNativeKeys
  >["manageNativeMailKey"];
  requireNativeSending: ReturnType<
    typeof createNativeKeys
  >["requireNativeSending"];
  configureNativeTransport: ReturnType<
    typeof createNativeSendTransport
  >["configureNativeTransport"];
  env: NativeMailPorts["env"];
}
export function createNativeSending(dependencies: NativeSendingDependencies) {
  const {
    resolveSessionReference,
    withAuthenticatedDatabase,
    queueMailConfiguration,
    byokPolicy,
    transaction,
    authorizeMail,
    manageNativeMailKey,
    requireNativeSending,
    configureNativeTransport,
    env,
  } = dependencies;
  async function actor(headers: Headers) {
    const session = await resolveSessionReference(headers);
    if (!session) mailUnavailable();
    return { subject: session.subject, sessionID: session.sessionID };
  }

  async function getNativeSending(headers: Headers, customerID: string) {
    return withAuthenticatedDatabase(headers, async () => {
      if (env.SPITZLI_NATIVE_MAIL_SENDING_ENABLED !== "true") return null;
      const identity = await actor(headers);
      let apiOrigin: string | null = null;
      if (env.MAIL_API_ORIGIN) {
        const url = new URL(env.MAIL_API_ORIGIN);
        if (
          url.protocol !== "https:" ||
          url.username ||
          url.password ||
          url.pathname !== "/" ||
          url.search ||
          url.hash
        )
          mailUnavailable();
        apiOrigin = url.origin;
      }
      return transaction(async (db) => {
        const access = await authorizeMail(
          db,
          identity,
          customerID,
          false,
          false,
        );
        if (!access.canManage) return null;
        const connection =
          (
            await db.query(
              "SELECT mode,state FROM webdock_mail.send_transport WHERE customer_id=$1",
              [customerID],
            )
          ).rows[0] ?? null;
        const submission = access.service?.desired_enabled
          ? (
              await db.query(
                "SELECT submission_state FROM webdock_mail.instance WHERE customer_id=$1",
                [customerID],
              )
            ).rows[0]
          : null;
        const state =
          connection?.state === "ready" && access.service?.desired_enabled
            ? (submission?.submission_state ?? "pending")
            : connection?.state;
        const transport = connection
          ? {
              mode: connection.mode,
              state,
              canConfigure:
                !["pending", "needs_review"].includes(connection.state) &&
                state !== "pending",
            }
          : null;
        const keys = (
          await db.query(
            'SELECT id,label,key_suffix AS suffix,revoked_at AS "revokedAt" FROM webdock_mail.api_key WHERE customer_id=$1 ORDER BY created_at DESC LIMIT 100',
            [customerID],
          )
        ).rows;
        const domains = (
          await db.query(
            "SELECT id,domain FROM webdock_auth.mail_sender_domain WHERE deleted_at IS NULL AND customer_id=$1 AND status='ownership_verified' ORDER BY domain",
            [customerID],
          )
        ).rows;
        return {
          customerID,
          transport,
          keys,
          domains,
          byokAllowed: (await byokPolicy(db, customerID)).turbosmtp,
          apiOrigin,
        };
      });
    });
  }

  async function manageNativeSending(
    headers: Headers,
    customerID: string,
    input: Record<string, string>,
  ) {
    return withAuthenticatedDatabase(headers, async () => {
      requireNativeSending();
      const identity = await actor(headers);
      if (input.action === "configure") {
        if (!["managed", "byok"].includes(input.mode)) mailUnavailable();
        await configureNativeTransport(
          identity,
          customerID,
          input.mode as "managed" | "byok",
        );
        await transaction(async (db) => {
          const access = await authorizeMail(
            db,
            identity,
            customerID,
            true,
            false,
          );
          if (access.service?.ready) {
            const row = (
              await db.query(
                "SELECT i.submission_state,i.submission_ready_revision,i.submission_transport_revision,t.revision FROM webdock_mail.instance i JOIN webdock_mail.send_transport t USING(customer_id) WHERE i.customer_id=$1",
                [customerID],
              )
            ).rows[0];
            if (
              row &&
              (row.submission_state !== "ready" ||
                String(row.submission_ready_revision) !==
                  String(access.service.revision) ||
                String(row.submission_transport_revision) !==
                  String(row.revision))
            )
              await queueMailConfiguration(
                db,
                identity,
                customerID,
                "submission.configure",
                customerID,
              );
          }
        });
        return { message: "Native Mail sending is configured." };
      }
      const command =
        input.action === "create"
          ? {
              action: "create",
              label: input.label,
              ...(input.domainID ? { domainID: input.domainID } : {}),
            }
          : { action: input.action, keyID: input.keyID };
      const result = await transaction((db) =>
        manageNativeMailKey(db, identity, customerID, command),
      );
      return {
        message: result.token
          ? "Save this Spitzli Cloud API key now. It is shown only once."
          : "Spitzli Cloud API key revoked.",
        ...("token" in result ? { token: result.token } : {}),
      };
    });
  }
  return { getNativeSending, manageNativeSending };
}
