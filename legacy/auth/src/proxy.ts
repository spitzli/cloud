import { NextResponse, type NextRequest } from "next/server";
import { legacyStudioRedirect } from "./lib/studio-links";

export function proxy(request: NextRequest) {
  const destination = legacyStudioRedirect(request.nextUrl, request.method);
  return destination ? NextResponse.redirect(destination, 307) : NextResponse.next();
}

export const config = {
  matcher: ["/admin/:path*", "/tenants/:path*", "/offers/:path*", "/people", "/sites"],
};
