import { mailOpenAPI } from "../../../../../../modules/mail/product/openapi";
export function GET() {
  return Response.json(mailOpenAPI(), {
    headers: { "Cache-Control": "public, max-age=300" },
  });
}
