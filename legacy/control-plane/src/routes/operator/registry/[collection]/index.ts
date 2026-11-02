import { readJSON } from "../../../../plugins/body";
import Elysia, { t } from "elysia";
import {
  registryService,
  registryIdentity,
} from "../../../../modules/registry/context";
const params = t.Object({
  collection: t.Union([
    t.Literal("customers"),
    t.Literal("projects"),
    t.Literal("audit-events"),
  ]),
});
export default new Elysia()
  .get(
    "",
    async ({ request, params }) => {
      const search = new URL(request.url).searchParams;
      const input: Record<string, unknown> = { collection: params.collection };
      for (const [key, value] of search) {
        if (search.getAll(key).length !== 1)
          throw Error("Duplicate query parameter");
        input[key] = ["page", "limit"].includes(key) ? Number(value) : value;
      }
      return (await registryService()).listRecords(
        registryIdentity(request),
        input,
      );
    },
    { params },
  )
  .post(
    "",
    async ({ request, params }) => {
      const body = await readJSON(request, 65_536);
      const service = await registryService(),
        actor = registryIdentity(request);
      if (params.collection === "customers")
        return service.writeCustomer(actor, null, body);
      if (params.collection === "projects")
        return service.writeProject(actor, null, body);
      return new Response(null, { status: 405 });
    },
    { params, parse: "none" },
  );
