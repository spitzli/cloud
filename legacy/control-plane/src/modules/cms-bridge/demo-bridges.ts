import { requireInstalledOperation } from "../../installation";
import { portalCall } from "../portal/request-context";
import { getStudioSession } from "../portal/request-context";
import { demoBridges } from "./demo-bridge-policy";
import { createCMSDemoBridges } from "@webdock/module-cms/server/demo-bridges";
export {
  type DemoRequest,
  type CalendarWindow,
  type CalendarSettings,
  type DemoAppointment,
  type CalendarState,
} from "@webdock/module-cms/server/demo-bridges";
export { demoBridges } from "./demo-bridge-policy";
export const { demoAccess, demoRequest, demoCalendarRequest } =
  createCMSDemoBridges({
    requireCMS: () => requireInstalledOperation("cms.access", []),
    getSites: () => portalCall("accountSites"),
    getStudioSession,
    demoBridges,
    fetch: (input, init) => globalThis.fetch(input, init),
  });
