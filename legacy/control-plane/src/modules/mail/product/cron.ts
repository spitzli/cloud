import { createMailCron } from "@spitzli/module-mail/server/product-cron";
import { withDatabaseScope } from "@spitzli/database-runtime";
import { mailJobs, runMailJob, type MailJob } from "./jobs";
export const { handleMailCron } = createMailCron({
  withDatabaseScope,
  runMailJob,
});
