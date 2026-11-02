import { APIError } from "better-auth/api";
import { authDatabase as database } from "../../db/runtime";
import { currentClaims } from "./authorization";

export function isOperatorOIDCClient(value: unknown) {
  const metadata = typeof value === "string" ? JSON.parse(value) : value;
  return Boolean(
    metadata &&
    typeof metadata === "object" &&
    Object.hasOwn(metadata, "webdock_operator_only"),
  );
}

/** Unmarked clients keep their existing authorization contract. */
export async function requireOperatorOIDCAccess(input: {
  clientID: string;
  subject?: string;
  sessionID?: string;
}) {
  const row = (
    await database.query(
      `SELECT c.metadata,c.disabled,b.id AS binding_id,b.organization_id,b.enabled,
       EXISTS(SELECT 1 FROM webdock_auth.session s WHERE s.id=$3
         AND s."userId"=$2 AND s."expiresAt">now()) AS session_valid
     FROM webdock_auth."oauthClient" c
     LEFT JOIN webdock_auth.app_binding b ON b.client_id=c."clientId"
     WHERE c."clientId"=$1`,
      [input.clientID, input.subject ?? null, input.sessionID ?? null],
    )
  ).rows[0];
  const deny = () => {
    throw new APIError("FORBIDDEN", {
      message: "Current authorized operator session is required.",
    });
  };
  if (!row) return deny();
  const metadata =
    typeof row.metadata === "string" ? JSON.parse(row.metadata) : row.metadata;
  if (!isOperatorOIDCClient(metadata)) return;
  if (
    metadata.webdock_operator_only !== true ||
    row.disabled ||
    !row.enabled ||
    row.organization_id ||
    typeof metadata.webdock_binding !== "string" ||
    metadata.webdock_binding !== row.binding_id ||
    !Array.isArray(metadata.webdock_allowed_subjects) ||
    !metadata.webdock_allowed_subjects.length ||
    !metadata.webdock_allowed_subjects.every(
      (id: unknown) => typeof id === "string" && /^[1-9][0-9]*$/.test(id),
    ) ||
    !input.subject ||
    !metadata.webdock_allowed_subjects.includes(input.subject) ||
    !row.session_valid
  )
    return deny();
  const claims = await currentClaims(input.subject, row.binding_id);
  if (claims.disabled || claims.webdock_role !== "operator") return deny();
  return claims;
}
