import { passkey } from "@better-auth/passkey";
import {
  APIError,
  createAuthMiddleware,
  getSessionFromCtx,
} from "better-auth/api";
import { twoFactor } from "better-auth/plugins";

// Password sign-ins use Better Auth's native MFA challenge. Passkey sign-ins
// instead require cryptographically verified device confirmation below.
export function passwordTwoFactor() {
  return twoFactor({ issuer: "Webdock", skipVerificationOnEnable: false });
}

export function passkeySecurity(baseURL: string) {
  const origin = new URL(baseURL);
  const plugin = passkey({
    rpID: origin.hostname,
    rpName: "Webdock",
    origin: origin.origin,
    authenticatorSelection: {
      residentKey: "required",
      userVerification: "required",
    },
    registration: {
      requireSession: true,
      afterVerification({ verification }) {
        // 1.7.7 requests verification but does not require it on the server.
        if (!verification.registrationInfo?.userVerified)
          throw new APIError("FORBIDDEN", {
            message:
              "Confirm your identity with your device PIN or biometrics.",
          });
      },
    },
    authentication: {
      async afterVerification({ ctx, verification, clientData }) {
        if (!verification.authenticationInfo.userVerified)
          throw new APIError("FORBIDDEN", {
            message:
              "Confirm your identity with your device PIN or biometrics.",
          });
        const credential = await ctx.context.adapter.findOne<{
          userId: string;
        }>({
          model: "passkey",
          where: [{ field: "credentialID", value: clientData.id }],
        });
        const user =
          credential &&
          (await ctx.context.internalAdapter.findUserById(credential.userId));
        if (!user?.emailVerified)
          throw new APIError("FORBIDDEN", {
            message: "Verify your email before signing in.",
          });
      },
    },
  });
  return {
    ...plugin,
    hooks: {
      before: [
        {
          matcher: (ctx: { path?: string }) =>
            [
              "/passkey/generate-register-options",
              "/passkey/verify-registration",
              "/passkey/delete-passkey",
            ].includes(ctx.path || ""),
          handler: createAuthMiddleware(async (ctx) => {
            const session = await getSessionFromCtx(ctx);
            if (!session)
              throw new APIError("UNAUTHORIZED", {
                message: "Sign in before managing passkeys.",
              });
            const user = session.user;
            if (
              !user.emailVerified ||
              user.mustChangePassword ||
              (user.role === "operator" && !user.twoFactorEnabled)
            )
              throw new APIError("FORBIDDEN", {
                message:
                  "Complete your account security setup before managing passkeys.",
              });
            if (ctx.path === "/passkey/delete-passkey") {
              // Every provisioned Webdock account retains password recovery. Do
              // not allow deletion if that fallback has since been removed.
              const account = await ctx.context.adapter.findOne<{
                password?: string;
              }>({
                model: "account",
                where: [
                  { field: "userId", value: user.id },
                  { field: "providerId", value: "credential" },
                ],
              });
              if (!account?.password)
                throw new APIError("FORBIDDEN", {
                  message: "Set a recovery password before removing a passkey.",
                });
            }
          }),
        },
      ],
      after: [
        {
          matcher: (ctx: { path?: string }) =>
            ctx.path === "/passkey/generate-authenticate-options",
          handler: createAuthMiddleware(async (ctx) => {
            const result = ctx.context.returned;
            if (
              result &&
              typeof result === "object" &&
              !(result instanceof APIError)
            )
              return ctx.json({ ...result, userVerification: "required" });
          }),
        },
      ],
    },
  };
}
