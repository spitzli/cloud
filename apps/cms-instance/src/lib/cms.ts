import { getPayload, type Payload } from "payload";
import config from "../payload.config.ts";
import { handleCMSRequest } from "@webdock/cms-ui/server";
import { modules } from "./content.ts";
import { loadCMSConfig } from "./config.ts";
import { validateRaster } from "./media.ts";
import { MEDIA_BYTES, METADATA_BYTES } from "@webdock/cms-bridge/types";
import { verifyMediaBinding } from "./media-storage.ts";
export const getCMS = async (): Promise<Payload> => {
  // Payload skips onInit on cached retries, including after failed initialization.
  await verifyMediaBinding(loadCMSConfig());
  return getPayload({ config });
};
export const cmsOptions = () => {
  const environment = loadCMSConfig();
  return {
    siteName: environment.siteName,
    siteURL: environment.origin,
    modules,
    locales: ["en", "de"],
    defaultLocale: "en",
  };
};
export const handleNativeCMS = (
  request: Request,
  trustedUser?: NonNullable<
    Awaited<ReturnType<typeof import("@webdock/cms-ui/server").cmsUser>>
  >,
) =>
  handleCMSRequest(request, cmsOptions(), getCMS, {
    ...(trustedUser ? { trustedUser } : {}),
    uploads: {
      allowFileReplacement: false,
      removeToTrash: true,
      maxFileBytes: MEDIA_BYTES,
      maxBodyBytes: MEDIA_BYTES + METADATA_BYTES,
      metadataBytes: METADATA_BYTES,
      validateFile: validateRaster,
    },
  });
