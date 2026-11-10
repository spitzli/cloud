import { configurePayloadSSO } from "@spitzli/payload-sso";
import { projectBridgeUser } from "@spitzli/cms-bridge/auth";
import { payloadEnvironment } from "./config.ts";
export const sso: ReturnType<typeof configurePayloadSSO> | null = process.env
  .SPITZLI_SSO_CLIENT_ID
  ? configurePayloadSSO({
      successPath: "/cms",
      logoutPath: "/cms",
      returnToPrefixes: ["/cms"],
      lookupUser: async (identity) => {
        const { getCMS } = await import("./cms.ts");
        const config = payloadEnvironment();
        const user = await projectBridgeUser(await getCMS(), {
          instance: config.instance,
          project: config.project,
          subject: identity.sub,
          role: identity.role,
          grantEpoch: "sso",
        });
        return {
          id: String(user.id),
          authSubject: identity.sub,
          role: user.role,
          name: String(user.name ?? identity.sub),
          email: String(user.email ?? ""),
          createdAt: String(user.createdAt ?? ""),
          updatedAt: String(user.updatedAt ?? ""),
        };
      },
    })
  : null;
