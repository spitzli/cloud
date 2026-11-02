import { readAuthProviderConfiguration } from "@webdock/auth-configuration";
import { legacyClerkMemberships } from "../auth/clerk-provider";
import { introspectToken } from "../auth/introspection";
import { configureOAuthSession } from "@webdock/oauth-session";
import { withDatabaseScope } from "@webdock/database-runtime";
import { database } from "../../db/runtime";
/** Read the existing mapping under the exact session verified by introspection. */
export function createPortalSession() {
  const clientID = process.env.WEBDOCK_SSO_CLIENT_ID!;
  const clientSecret = process.env.WEBDOCK_SSO_CLIENT_SECRET!;
  return configureOAuthSession({
    introspect: async (token) => {
      const response = await introspectToken(token, clientID, clientSecret);
      if (!response.ok)
        throw Error("Central sign-in is temporarily unavailable");
      return response.json();
    },
    successPath: "/",
    logoutPath: "/login",
    centralLogout: true,
    allowUnmappedPortalUsers: true,
    returnToPrefixes: [
      "/tenants",
      "/offers",
      "/people",
      "/sites",
      "/customers",
      "/projects",
      "/activity",
      "/integrations",
      "/infrastructure",
      "/hosting",
      "/operations",
    ],
    lookupUser: async (identity) => {
      if (!identity.sessionID || identity.role !== "operator") return null;
      const provider = readAuthProviderConfiguration(process.env.WEBDOCK_AUTH_PROVIDER).provider;
      const memberships = provider === "clerk" ? await legacyClerkMemberships(identity.sub) : undefined;
      if (provider === "clerk" && (!memberships?.operator || !memberships.twoFactorEnabled)) return null;
      return withDatabaseScope(
        {
          kind: "session",
          authProvider: provider,
          ...(memberships ? {memberships:{...memberships,subject:identity.sub,sessionID:identity.sessionID}} : {}),
          subject: identity.sub,
          sessionID: identity.sessionID,
        },
        async () => {
          if (process.env.DATABASE_RLS_SECRET)
            return (
              await database.query("SELECT webdock_rls.portal_user() AS value")
            ).rows[0].value;
          const result = await database.query(
            `SELECT id,name,email,role,auth_subject AS "authSubject",created_at AS "createdAt",updated_at AS "updatedAt" FROM webdock_admin.users WHERE auth_subject=$1 AND ($2::boolean OR role='operator') LIMIT 2`,
            [identity.sub, provider === "clerk"],
          );
          if (result.rows.length !== 1) return null;
          const row = result.rows[0];
          return {
            ...row,
            role: "operator",
            createdAt: new Date(row.createdAt).toISOString(),
            updatedAt: new Date(row.updatedAt).toISOString(),
          };
        },
      );
    },
  });
}
