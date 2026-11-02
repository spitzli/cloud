import { readAuthProviderConfiguration } from "@webdock/auth-configuration";
import { legacyClerkMemberships } from "./clerk-provider";
import { createBetterAuthRuntime } from "@webdock/auth-better-auth/runtime";
import { admin, organization, jwt } from "better-auth/plugins";
import { adminAc, userAc } from "better-auth/plugins/admin/access";
import { oauthProvider } from "@better-auth/oauth-provider";
import {
  createAuthMiddleware,
  APIError,
  getSessionFromCtx,
} from "better-auth/api";
import { authDatabase as defaultDatabase } from "../../db/runtime";
import type { Pool } from "pg";
import { drizzle } from "drizzle-orm/node-postgres";
import { drizzleAdapter } from "better-auth/adapters/drizzle";
import * as schema from "../../db/schema/auth";
import { guardOrganizationRequest } from "./tenant-policy";
import { cookiePolicy } from "./cookie-policy";
import { offlineProvisioning } from "./offline";
import { currentClaims } from "./authorization";
import {
  isOperatorOIDCClient,
  requireOperatorOIDCAccess,
} from "./operator-oidc";
import { sendAuthMail } from "./mail";
import { passkeySecurity, passwordTwoFactor } from "./passkeys";
import {
  mcpResource,
  mcpScopes,
  currentMCPClaims,
  hostingScopes,
  currentHostingMCPClaims,
} from "./mcp";
export function createAuthentication(options: {
  database: Pool;
  baseURL: string;
  secret: string;
  adapter?: "pg" | "drizzle";
}) {
  const baseURL = options.baseURL;
  const authDatabase = options.database;
  async function legacyLoginAllowed(userID:string){
    if(!(await authDatabase.query("SELECT to_regclass('webdock_auth.clerk_identity') IS NOT NULL AS present")).rows[0]?.present)return true;
    return (await authDatabase.query('SELECT NOT EXISTS(SELECT 1 FROM webdock_auth.clerk_identity WHERE user_id=$1 AND NOT legacy_eligible) AS allowed',[userID])).rows[0]?.allowed===true;
  }
  const auth = createBetterAuthRuntime(() => ({
      appName: "Webdock",
      baseURL,
      basePath: "/api/auth",
      secret: options.secret,
      database:
        options.adapter === "pg"
          ? authDatabase
          : drizzleAdapter(drizzle({ client: authDatabase }), {
              provider: "pg",
              schema,
              transaction: true,
            }),
      databaseHooks:{
        account:{create:{before:async(account)=>{if(!(await legacyLoginAllowed(account.userId)))throw new APIError("FORBIDDEN",{message:"Use your Console account to sign in."});return {data:account};}}},
        session:{create:{before:async(session)=>{if(!(await legacyLoginAllowed(session.userId)))throw new APIError("FORBIDDEN",{message:"Use your Console account to sign in."});return {data:session};}}},
      },
      trustedOrigins: [baseURL],
      disabledPaths: ["/token", "/admin/impersonate-user"],
      advanced: {
        disableOriginCheck: false,
        database: { generateId: false },
        ...cookiePolicy(baseURL),
        ipAddress: { ipAddressHeaders: ["x-webdock-client-ip"] },
      },
      session: {
        expiresIn: 60 * 60 * 8,
        updateAge: 60 * 15,
        cookieCache: { enabled: false },
      },
      rateLimit: {
        enabled: true,
        storage: "database",
        window: 60,
        max: 200,
        customRules: {
          "/sign-in/email": { window: 60, max: 10 },
          "/two-factor/verify-totp": { window: 60, max: 8 },
          "/two-factor/verify-backup-code": { window: 60, max: 5 },
        },
      },
      emailAndPassword: {
        enabled: true,
        disableSignUp: true,
        requireEmailVerification: true,
        minPasswordLength: 12,
        maxPasswordLength: 128,
        revokeSessionsOnPasswordReset: true,
        onPasswordReset: async ({ user }) => {
          // The one-time reset link proves mailbox access for a provisioned invitee.
          await authDatabase.query(
            'UPDATE webdock_auth."user" SET "emailVerified"=true,"mustChangePassword"=false WHERE id=$1 AND "mustChangePassword"=true',
            [user.id],
          );
        },
        sendResetPassword: async ({ user, url }) => {
          if(!(await legacyLoginAllowed(user.id)))return;
          return sendAuthMail({
            to: user.email,
            subject:
              "mustChangePassword" in user && user.mustChangePassword === true
                ? "Set up your Webdock account"
                : "Reset your Webdock password",
            text:
              "mustChangePassword" in user && user.mustChangePassword === true
                ? `Your Webdock administrator has invited you. Choose your password to set up your account:\n${url}\nIf you were not expecting an invitation, you can ignore this message.`
                : `Reset your Webdock password using this link:\n${url}\nIf you did not request this, you can ignore this message.`,
          });
        },
      },
      emailVerification: {
        sendVerificationEmail: async ({ user, url }) =>
          sendAuthMail({
            to: user.email,
            subject: "Verify your Webdock email",
            text: `Confirm your email using this link:\n${url}`,
          }),
      },
      user: {
        additionalFields: {
          mustChangePassword: {
            type: "boolean",
            defaultValue: false,
            input: false,
          },
        },
        deleteUser: { enabled: false },
      },
      hooks: {
        before: createAuthMiddleware(async (ctx) => {
          if (readAuthProviderConfiguration(process.env.WEBDOCK_AUTH_PROVIDER).provider === "clerk" &&
            (ctx.path.startsWith("/admin/") || ctx.path.startsWith("/organization/")))
            throw new APIError("FORBIDDEN", { message: "Manage accounts, roles and team memberships in Clerk." });
          if (
            (ctx.path.startsWith("/admin/") ||
              ctx.path.startsWith("/organization/")) &&
            !offlineProvisioning.getStore()
          ) {
            const session = await getSessionFromCtx(ctx);
            await guardOrganizationRequest(
              ctx.path,
              ctx.body,
              ctx.query,
              session?.user.id,
            );
            if (
              (ctx.path.startsWith("/admin/") &&
                (!session || session.user.role !== "operator")) ||
              (session?.user.role === "operator" &&
                (session.user.mustChangePassword ||
                  !session.user.twoFactorEnabled))
            )
              throw new APIError("FORBIDDEN", {
                message: "Complete operator security setup first.",
              });
          }
        }),
        after: createAuthMiddleware(async (ctx) => {
          if (ctx.path === "/oauth2/introspect") {
            const result = ctx.context.returned;
            if (
              result &&
              typeof result === "object" &&
              !(result instanceof APIError) &&
              "active" in result &&
              result.active === true &&
              "client_id" in result &&
              typeof result.client_id === "string"
            ) {
              const client = await ctx.context.adapter.findOne<{
                metadata?: unknown;
              }>({
                model: "oauthClient",
                where: [{ field: "clientId", value: result.client_id }],
              });
              if (isOperatorOIDCClient(client?.metadata))
                try {
                  await requireOperatorOIDCAccess({
                    clientID: result.client_id,
                    subject:
                      "sub" in result && typeof result.sub === "string"
                        ? result.sub
                        : undefined,
                    sessionID:
                      "sid" in result && typeof result.sid === "string"
                        ? result.sid
                        : undefined,
                  });
                } catch (error) {
                  if (error instanceof APIError && error.status === "FORBIDDEN")
                    return ctx.json({ active: false });
                  throw error;
                }
            }
          }

          if (
            ctx.path === "/two-factor/verify-totp" &&
            ctx.context.session?.user.twoFactorEnabled === false &&
            ctx.context.newSession?.user.twoFactorEnabled === true
          )
            await authDatabase.query(
              'DELETE FROM webdock_auth.session WHERE "userId"=$1 AND id<>$2',
              [
                ctx.context.newSession.user.id,
                ctx.context.newSession.session.id,
              ],
            );
          if (
            ctx.path === "/change-password" &&
            ctx.context.session?.user.id &&
            !(ctx.context.returned instanceof APIError)
          )
            await authDatabase.query(
              'UPDATE webdock_auth."user" SET "mustChangePassword"=false WHERE id=$1',
              [ctx.context.session.user.id],
            );
        }),
      },
      plugins: [
        admin({
          defaultRole: "user",
          adminRoles: ["operator"],
          roles: { operator: adminAc, user: userAc },
        }),
        organization({
          allowUserToCreateOrganization: async (user) => {
            const row = (
              await authDatabase.query(
                'SELECT role FROM webdock_auth."user" WHERE id=$1',
                [user.id],
              )
            ).rows[0];
            return row?.role === "operator";
          },
          disableOrganizationDeletion: true,
          requireEmailVerificationOnInvitation: true,
          sendInvitationEmail: async (data) => {
            const user = (
              await authDatabase.query(
                'SELECT "mustChangePassword","emailVerified" FROM webdock_auth."user" WHERE lower(email)=lower($1)',
                [data.email],
              )
            ).rows[0];
            if (user?.mustChangePassword && !user.emailVerified) {
              await auth.api.requestPasswordReset({
                body: {
                  email: data.email,
                  redirectTo: `${baseURL}/reset-password?invitation=${encodeURIComponent(data.id)}`,
                },
              });
            } else
              await sendAuthMail({
                to: data.email,
                subject: `Invitation to ${data.organization.name} on Webdock`,
                text: `You have been invited to ${data.organization.name}.\n${baseURL}/invitation?id=${encodeURIComponent(data.id)}`,
              });
          },
        }),
        passwordTwoFactor(),
        passkeySecurity(baseURL),
        jwt({
          disableSettingJwtHeader: true,
          jwks: { keyPairConfig: { alg: "RS256" } },
        }),
        oauthProvider({
          loginPage: "/sign-in",
          consentPage: "/consent",
          scopes: [
            "openid",
            "profile",
            "email",
            "offline_access",
            ...mcpScopes,
            ...hostingScopes,
          ],
          resources: [
            {
              identifier: mcpResource,
              name: "Webdock Studio",
              allowedScopes: [...mcpScopes, ...hostingScopes, "offline_access"],
              accessTokenTtl: 300,
            },
          ],
          enforcePerClientResources: true,
          grantTypes: ["authorization_code", "refresh_token"],
          refreshTokenExpiresIn: 60 * 60 * 8,
          // 1.7.7 discovers public PKCE support from this flag, even for manually
          // provisioned clients. Keep this false/true pair: DCR remains disabled
          // (checked first by /register), while discovery correctly advertises none.
          allowDynamicClientRegistration: false,
          allowUnauthenticatedClientRegistration: true,
          accessTokenExpiresIn: 28800,
          idTokenExpiresIn: 300,
          rateLimit: { introspect: { window: 60, max: 600 } },
          clientPrivileges: async ({ user, action }) => {
            if (action === "create") return Boolean(offlineProvisioning.getStore());
            if (readAuthProviderConfiguration(process.env.WEBDOCK_AUTH_PROVIDER).provider === "clerk") {
              if (!user?.id || user.banned === true) return false;
              try {
                const proof = await legacyClerkMemberships(user.id);
                return proof?.operator === true && proof.twoFactorEnabled === true;
              } catch { return false; }
            }
            return user?.role === "operator" && user.twoFactorEnabled === true && user.mustChangePassword !== true;
          },
          postLogin: {
            page: "/account",
            consentReferenceId: () => undefined,
            shouldRedirect: async ({ user }) => {
              if (readAuthProviderConfiguration(process.env.WEBDOCK_AUTH_PROVIDER).provider === "clerk") {
                try {
                  const proof = await legacyClerkMemberships(user.id);
                  return !proof || (proof.operator && !proof.twoFactorEnabled);
                } catch { return true; }
              }
              return user.mustChangePassword === true || (user.role === "operator" && user.twoFactorEnabled !== true);
            },
          },
          generateClientId: () => {
            const id = offlineProvisioning.getStore()?.clientID;
            if (!id)
              throw new APIError("FORBIDDEN", {
                message:
                  "Clients are registered through the operator provisioning tool.",
              });
            return id;
          },
          extensions: [
            {
              claims: {
                // Opaque access tokens skip accessToken hooks in provider1.7.7.
                idToken: async ({ client, metadata, user, sessionId }) => {
                  if (isOperatorOIDCClient(metadata))
                    await requireOperatorOIDCAccess({
                      clientID: client.clientId,
                      subject: user?.id,
                      sessionID: sessionId,
                    });
                  return {};
                },
                accessToken: async ({
                  ctx,
                  metadata,
                  grantType,
                  sessionId,
                  user,
                }) => {
                  if (
                    (metadata?.webdock_mcp === true ||
                      metadata?.webdock_hosting === true) &&
                    grantType
                  ) {
                    // Offline refresh tokens survive session deletion with a NULL FK.
                    // Never let renewal turn session-bound access into sessionless access.
                    const session = sessionId
                      ? await ctx.context.adapter.findOne<{
                          userId: string;
                          expiresAt: Date;
                        }>({
                          model: "session",
                          where: [{ field: "id", value: sessionId }],
                        })
                      : null;
                    if (
                      !session ||
                      session.userId !== user?.id ||
                      new Date(session.expiresAt).getTime() <= Date.now()
                    )
                      throw new APIError("FORBIDDEN", {
                        message: "Sign in again to reconnect this client.",
                      });
                  }
                  return {};
                },
              },
            },
          ],
          customTokenResponseFields: async ({
            metadata,
            user,
            grantType,
            scopes,
          }) => {
            if (
              isOperatorOIDCClient(metadata) &&
              (grantType !== "authorization_code" || !scopes.includes("openid"))
            )
              throw new APIError("FORBIDDEN", {
                message: "An interactive OpenID login is required.",
              });
            if (
              metadata?.webdock_hosting === true &&
              (await currentHostingMCPClaims(user?.id)).disabled
            )
              throw new APIError("FORBIDDEN", {
                message: "Current hosting authorization is required.",
              });
            if (
              metadata?.webdock_mcp === true &&
              (await currentMCPClaims(user?.id)).disabled
            )
              throw new APIError("FORBIDDEN", {
                message: "Current operator authorization is required.",
              });
            return {};
          },
          customAccessTokenClaims: ({ user, metadata }) =>
            metadata?.webdock_hosting === true
              ? currentHostingMCPClaims(user?.id)
              : metadata?.webdock_mcp === true
                ? currentMCPClaims(user?.id)
                : currentClaims(user?.id, metadata?.webdock_binding),
        }),
      ],
    }));

  return auth;
}
export const auth = createAuthentication({
  database: defaultDatabase,
  baseURL: process.env.BETTER_AUTH_URL || "http://localhost:3125",
  secret: process.env.BETTER_AUTH_SECRET || "",
});
