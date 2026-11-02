import { runOperator, requireApply } from "./context";
import {
  writeCustomer,
  writeProject,
  writeInstance,
} from "../../src/modules/registry/compat";
requireApply();
await runOperator(async (actor) => {
  const p = actor.payload,
    user = actor.user;
  const customer = async (name: string) =>
    (
      await p.find({
        collection: "customers",
        where: { name: { equals: name } },
        limit: 1,
        depth: 0,
      })
    ).docs[0] || (await writeCustomer(actor, null, { name }));
  const spitzli = await customer("Spitzli Development"),
    stall = await customer("Stall Eichenbruch");
  const entries = [
    {
      name: "Webdock",
      customer: spitzli.id,
      url: "https://webdock.dev",
      repositoryURL: "https://github.com/spitzli/webdock",
      adminURL: "https://webdock.dev/admin",
      schemaName: "webdock",
      providerProjectID: "prj_TqeFXSoCLV7dNDQ2VHdRiYnR4K7L",
      template: "webdock-landing" as const,
    },
    {
      name: "Spitzli Development",
      customer: spitzli.id,
      url: "https://spitzli.dev",
      repositoryURL: "https://github.com/spitzli/spitzli",
      adminURL: "https://spitzli.vercel.app/admin",
      schemaName: "spitzli",
      providerProjectID: "prj_V5kkIarQ82smsG8oNaXHN1gwKrgc",
      template: "spitzli-portfolio" as const,
    },
    {
      name: "Stall Eichenbruch",
      customer: stall.id,
      url: "https://www.stall-eichenbruch.de",
      repositoryURL: "https://github.com/spitzli/stall-eichenbruch",
      adminURL: "https://www.stall-eichenbruch.de/admin",
      schemaName: "stall",
      providerProjectID: "prj_I6CM5NhcNs922GUDx1aU67TlEsCq",
      template: "stall-business" as const,
    },
  ];
  for (const entry of entries) {
    const existing = await p.find({
      collection: "cms-instances",
      where: { providerProjectID: { equals: entry.providerProjectID } },
      limit: 2,
      depth: 0,
    });
    if (existing.totalDocs) {
      if (existing.totalDocs !== 1) throw Error("Ambiguous existing instance");
      continue;
    }
    const found = await p.find({
      collection: "projects",
      where: {
        and: [
          { name: { equals: entry.name } },
          { customer: { equals: entry.customer } },
        ],
      },
      limit: 2,
      depth: 0,
    });
    if (found.totalDocs > 1) throw Error("Ambiguous existing project");
    const project =
      found.docs[0] ||
      (await writeProject(actor, null, {
        name: entry.name,
        customer: entry.customer,
        url: entry.url,
        repositoryURL: entry.repositoryURL,
      }));
    await writeInstance(
      actor,
      null,
      {
        label: entry.name,
        project: project.id,
        adminURL: entry.adminURL,
        schemaName: entry.schemaName,
        providerProjectID: entry.providerProjectID,
        template: entry.template,
        payloadVersion: "4.0.0-canary.37",
        status: "active",
        provider: "vercel",
      },
      true,
    );
  }
  console.log(
    "Existing CMS records verified/registered through the signed API services.",
  );
});
