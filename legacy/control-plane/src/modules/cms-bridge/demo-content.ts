import { demoAccess } from "./demo-bridges";
import { createCMSDemoContent } from "@webdock/module-cms/server/demo-content";
export {
  type WebsiteContentField,
  type ContentVariant,
} from "@webdock/module-cms/server/demo-content";
export const { websiteContent, saveWebsiteContent } = createCMSDemoContent({
  demoAccess,
  fetch: (input, init) => globalThis.fetch(input, init),
});
