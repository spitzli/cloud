import { acceptProviderEvents } from "../../../../../../modules/mail/product/callback-inbox";
export async function POST(
  request: Request,
  context: { params: Promise<{ sourceID: string }> },
) {
  return acceptProviderEvents((await context.params).sourceID, request);
}
