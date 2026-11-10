import { headers } from "next/headers";
import { redirect } from "next/navigation";
import { cmsUser } from "@spitzli/cms-ui/server";
import { CMSApp } from "@spitzli/cms-ui/client";
import { getCMS, cmsOptions } from "../../lib/cms.ts";
import { sso } from "../../lib/sso.ts";
export const dynamic = "force-dynamic";
export default async function Page() {
  const user = await cmsUser(await getCMS(), await headers());
  if (!user) {
    if (sso) redirect("/api/sso/login?returnTo=/cms");
    return (
      <main>
        <h1>Open your project CMS</h1>
        <p>Open this editor through your project in the Webdock Console.</p>
      </main>
    );
  }
  const options = cmsOptions();
  const issuer = process.env.SPITZLI_AUTH_ISSUER ?? options.siteURL;
  return (
    <CMSApp
      siteName={options.siteName}
      siteURL={options.siteURL}
      accountURL={new URL("/account", issuer).href}
      accessURL={new URL("/people", issuer).href}
    />
  );
}
