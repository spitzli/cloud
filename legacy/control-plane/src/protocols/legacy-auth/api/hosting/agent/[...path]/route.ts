import { hostingAgentAPI } from "../../../../../../modules/hosting/agent-api";
export const runtime = "nodejs";
export async function POST(
  request: Request,
  { params }: { params: Promise<{ path: string[] }> },
) {
  return hostingAgentAPI(request, (await params).path);
}
