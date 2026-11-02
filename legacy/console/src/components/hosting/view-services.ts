import "server-only";
import { requireRegistryOperator } from "@/lib/registry-page";
export async function getAvailableProjects(customerID: string) {
  const { payload, user } = await requireRegistryOperator();
  const choices = await payload.find({
    collection: "projects",
    where: {
      and: [
        { customer: { equals: customerID } },
        { status: { equals: "active" } },
      ],
    },
    limit: 100,
    depth: 0,
    user,
    overrideAccess: false,
  });
  return choices.docs.map((p) => ({ id: String(p.id), name: p.name }));
}
