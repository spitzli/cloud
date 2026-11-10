import { createCMSBridgeHandler } from "@spitzli/cms-bridge/server";
import { PostgresBridgeTransfers } from "@spitzli/cms-bridge/postgres";
import {
  handleCMSRequest,
  type CMSRequestAdapter,
} from "@spitzli/cms-ui/server";
import { cmsOptions, getCMS } from "@/lib/cms";

export const runtime = "nodejs";
const handle = createCMSBridgeHandler({
  getConfig: () => {
    const secret = process.env.CMS_BRIDGE_SECRET;
    const instance = process.env.CMS_INSTANCE_ID;
    const project = process.env.CMS_PROJECT_ID;
    if (!secret || !instance || !project) return null;
    return {
      secret,
      instance,
      project,
      origin: new URL(cmsOptions.siteURL).origin,
    };
  },
  getPayload: getCMS,
  createTransfers: () =>
    new PostgresBridgeTransfers({
      connect: async () => (await getCMS()).db.pool.connect(),
    }),
  handleCMS: (request, trustedUser) =>
    handleCMSRequest(request, cmsOptions, getCMS, {
      trustedUser: trustedUser as CMSRequestAdapter["trustedUser"],
    }),
});
export { handle as GET, handle as POST };
