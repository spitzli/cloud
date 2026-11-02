import { legacyClerkMemberships } from "./clerk-provider";
import { ConsoleAuthError } from "./clerk-identity";
async function linkedUserProof(subject: string) {
  try {
    return await legacyClerkMemberships(subject);
  } catch (error) {
    if (error instanceof ConsoleAuthError && error.status === 403) return false;
    throw error;
  }
}
import { authDatabase as database } from "../../db/runtime";
export const mcpResource =
  process.env.WEBDOCK_MCP_RESOURCE || "https://studio.webdock.dev/api/mcp";
export const mcpScopes = ["webdock:read", "webdock:write"] as const;

// Re-evaluate operator authorization on every resource request, including JWTs.
export async function currentMCPClaims(subject: string | undefined) {
  if (!subject) return { disabled: true };
  const proof = await linkedUserProof(subject);
  if (proof === false) return { disabled: true };
  const { rows } = await database.query(
    `SELECT role,banned,"emailVerified","twoFactorEnabled","mustChangePassword" FROM webdock_auth."user" WHERE id=$1`,
    [subject],
  );
  const user = rows[0];
  return user &&
    (proof ? proof.operator : user.role === "operator") &&
    !user.banned &&
    (proof || user.emailVerified) &&
    (proof ? proof.twoFactorEnabled : user.twoFactorEnabled) &&
    !user.mustChangePassword
    ? { webdock_role: "operator", disabled: false }
    : { disabled: true };
}

export const hostingScopes = ["hosting:read", "hosting:write"] as const;
// Hosting clients never inherit the operator-only registry claims.
export async function currentHostingMCPClaims(subject: string | undefined) {
  if (!subject) return { disabled: true };
  const proof = await linkedUserProof(subject);
  if (proof === false) return { disabled: true };
  const user = (
    await database.query(
      'SELECT role,banned,"emailVerified","twoFactorEnabled","mustChangePassword" FROM webdock_auth."user" WHERE id=$1',
      [subject],
    )
  ).rows[0];
  if (
    !user ||
    user.banned ||
    (!proof && !user.emailVerified) ||
    user.mustChangePassword ||
    (!proof && !["user", "operator"].includes(user.role)) ||
    (proof
      ? proof.operator && !proof.twoFactorEnabled
      : user.role === "operator" && !user.twoFactorEnabled)
  )
    return { disabled: true };
  return {
    disabled: false,
    webdock_hosting: true,
    webdock_role: (proof ? proof.operator : user.role === "operator")
      ? "hosting-operator"
      : "hosting-customer",
  };
}
