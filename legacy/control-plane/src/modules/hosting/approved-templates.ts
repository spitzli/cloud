import { configureApprovedAppTemplates } from "./plugin-provisioning-runtime";
import { managedCMSTemplate } from "./plugin-template-runtime";
/** Both the API and setup worker must load the same operator-owned approval. */
export function configureOperatorAppTemplates(raw = process.env.WEBDOCK_APPROVED_CMS_TEMPLATE) {
  if (!raw) return undefined;
  const approved = managedCMSTemplate(JSON.parse(raw));
  return configureApprovedAppTemplates(approved.templates, approved.hooks);
}
