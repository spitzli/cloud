import { CustomerHosting } from "@/components/hosting/customer";
export default async function Hosting({
  params,
  searchParams,
}: {
  params: Promise<{ customerID: string }>;
  searchParams: Promise<{ page?: string; q?: string; sort?: string }>;
}) {
  const { customerID } = await params;
  const q = await searchParams;
  return (
    <CustomerHosting
      customerID={customerID}
      path={`/tenants/${customerID}/hosting`}
      page={/^[1-9][0-9]{0,3}$/.test(q.page ?? "") ? Number(q.page) : 1}
      sort={q.sort === "-name" ? "-name" : "name"}
      search={(q.q ?? "").slice(0, 160)}
    />
  );
}
