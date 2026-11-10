import { notFound, redirect } from "next/navigation";
import { headers } from "next/headers";
import { cmsUser } from "@spitzli/cms-ui/server";
import { getCMS } from "../../../../lib/cms.ts";
import { contentLocale } from "../../../../lib/public.ts";
import { PageView } from "../../../../lib/page-view.tsx";
export const dynamic = "force-dynamic";
export const metadata = { robots: { index: false, follow: false } };
export default async function Preview({
  params,
}: {
  params: Promise<{ locale: string; slug: string }>;
}) {
  const { locale: requested, slug } = await params;
  const locale = contentLocale(requested);
  if (!locale || !/^[a-z0-9-]{1,120}$/.test(slug)) notFound();
  const payload = await getCMS();
  const user = await cmsUser(payload, await headers());
  if (!user) redirect("/api/sso/login?returnTo=/cms");
  const result = await payload.find({
    collection: "pages",
    overrideAccess: false,
    user,
    draft: true,
    depth: 1,
    locale,
    fallbackLocale: false,
    limit: 1,
    where: { slug: { equals: slug } },
  });
  if (!result.docs[0]) notFound();
  return (
    <>
      <p>Authenticated draft preview</p>
      <PageView page={result.docs[0]} />
    </>
  );
}
