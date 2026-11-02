import {
  demoBridges as parseBridges,
  cmsAllowedOrigins,
} from "@webdock/module-cms/server/demo-bridge-policy";
export {
  authorizedDemoSite,
  cmsAllowedOrigins,
  type DemoBridge,
} from "@webdock/module-cms/server/demo-bridge-policy";
export function demoBridges(raw = process.env.WEBDOCK_DEMO_BRIDGES || "{}") {
  return parseBridges(
    raw,
    cmsAllowedOrigins(process.env.WEBDOCK_CMS_ALLOWED_ORIGINS || "[]"),
  );
}
