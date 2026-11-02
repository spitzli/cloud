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
  id: t.String({ pattern: "^[1-9][0-9]{0,18}$" }),
});
export default new Elysia()
  .get(
    "",
    async ({ request, params }) =>
      (await registryService()).getRecord(
        registryIdentity(request),
        params.collection,
        params.id,
      ),
    { params },
  )
  .put(
    "",
    async ({ request, params }) => {
      const body = await readJSON(request, 65_536);
      const service = await registryService(),
        actor = registryIdentity(request);
      if (params.collection === "customers")
        return service.writeCustomer(actor, params.id, body);
      if (params.collection === "projects")
        return service.writeProject(actor, params.id, body);
      return new Response(null, { status: 405 });
    },
    { params, parse: "none" },
  )
  .patch(
    "",
    async ({ request, params }) => {
      const body = await readJSON(request, 65_536);
      if (params.collection !== "customers" && params.collection !== "projects")
        return new Response(null, { status: 405 });
      if (
        !body ||
        typeof body !== "object" ||
        Array.isArray(body) ||
        Object.keys(body).length !== 1 ||
        typeof (body as { archived?: unknown }).archived !== "boolean"
      )
        return new Response(null, { status: 400 });
      return (await registryService()).archive(
        registryIdentity(request),
        params.collection,
        params.id,
        (body as { archived: boolean }).archived,
      );
    },
    { params, parse: "none" },
  );
