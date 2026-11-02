import { requireInstalledOperation } from "../../installation";
import { demoAccess, demoRequest, demoCalendarRequest } from "./demo-bridges";
import { demoBridges } from "./demo-bridge-policy";
import { canvasList, canvasPage, writeCanvas } from "./canvas-bridge";
import { websiteContent, saveWebsiteContent } from "./demo-content";
import { shopAccess, shopRequest } from "./studio-shop";
import { portalCall } from "../portal/request-context";
import { createCMSDispatch } from "@webdock/module-cms/server/dispatch";
export { cmsShapes } from "@webdock/module-cms/server/dispatch";
export const { dispatchCMS } = createCMSDispatch({
  requireCMS: () => requireInstalledOperation("cms.access", []),
  demoAccess,
  demoRequest,
  demoCalendarRequest,
  demoBridges,
  canvasList,
  canvasPage,
  writeCanvas,
  websiteContent,
  saveWebsiteContent,
  shopAccess,
  shopRequest,
  getSites: () => portalCall("accountSites"),
});
