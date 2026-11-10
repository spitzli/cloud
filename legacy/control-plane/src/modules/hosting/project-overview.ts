import { authorizeHosting } from "./authorization";
import { readServiceBindings } from "./service-bindings";
import { readHostingView } from "./view-cache";
import { createHostingProjectOverview } from "@spitzli/module-hosting/server/project-overview";
export const { readProjectOverview } = createHostingProjectOverview({
  authorizeHosting,
  readServiceBindings,
  readHostingView,
});
