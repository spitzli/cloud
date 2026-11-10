import { createCMSBridgeHandler } from "@spitzli/cms-bridge/server";
import { loadCMSConfig } from "../../../../lib/config.ts";
import { getCMS, handleNativeCMS } from "../../../../lib/cms.ts";
export const runtime = "nodejs";
export const dynamic = "force-dynamic";
const handle = createCMSBridgeHandler({
  getConfig: () => {
    const config = loadCMSConfig();
    return {
      secret: config.bridgeSecret,
      instance: config.instance,
      project: config.project,
      origin: config.origin,
      transferDirectory: config.transferDirectory,
    };
  },
  getPayload: getCMS,
  handleCMS: (request, user) => handleNativeCMS(request, user as never),
});
export const GET = handle;
export const POST = handle;
