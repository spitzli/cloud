import { notFound, redirect } from "next/navigation";
import { safeAdminReturnTo } from "@spitzli/payload-sso/return-to";
import { HostingError } from "@spitzli/hosting-contracts";

/** Only absent/expired local credentials can automatically start a new login. */
export class HostingSessionRequiredError extends HostingError {
  constructor() { super(401, "Sign in to Studio to continue."); }
}

/** Page-only recovery: API responses and form mutation errors keep their original status. */
export async function withHostingPageAccess<T>(
  returnTo: string,
  load: () => Promise<T>,
): Promise<T> {
  try {
    return await load();
  } catch (error) {
    if (error instanceof HostingError) {
      if (error instanceof HostingSessionRequiredError) {
        const path =
          safeAdminReturnTo(returnTo, [
            "/hosting",
            "/tenants",
            "/customers",
            "/infrastructure",
          ]) ?? "/tenants";
        redirect("/api/sso/login?returnTo=" + encodeURIComponent(path));
      }
      if (error.status === 403 || error.status === 404) notFound();
    }
    throw error;
  }
}
