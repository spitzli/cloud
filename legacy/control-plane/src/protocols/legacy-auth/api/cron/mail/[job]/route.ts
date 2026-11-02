import { handleMailCron } from "../../../../../../modules/mail/product/cron";
export const maxDuration = 60;
export async function GET(
  request: Request,
  context: { params: Promise<{ job: string }> },
) {
  return handleMailCron(request, (await context.params).job);
}
