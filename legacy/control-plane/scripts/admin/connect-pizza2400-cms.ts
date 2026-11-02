import { runOperator, requireApply } from "./context";
requireApply();
// Register only the already-deployed, verified instance; no provisioning side effects here.
import fs from "node:fs";
import { parseEnv } from "node:util";
import { writeInstance, writeProject } from "../../src/modules/registry/compat";
const registry = JSON.parse(
  fs.readFileSync("../../../pizza2400/.provisioning.json", "utf8"),
);
const siteEnvironment = parseEnv(
  fs.readFileSync("../../../pizza2400/.env.deploy", "utf8"),
);
const configuredURL = siteEnvironment.WEBDOCK_STUDIO_URL;
if (
  !configuredURL ||
  new URL(configuredURL).origin !== "https://studio.webdock.dev"
)
  throw Error("Central Webdock management URL required");
const bindingPath = new URL(configuredURL).pathname.match(
  /^\/sites\/(\d+)(?:\/|$)/,
);
if (!bindingPath) throw Error("Central Webdock site binding required");
const managementURL = "https://studio.webdock.dev/sites/" + bindingPath[1];
const hosting = JSON.parse(
  fs.readFileSync("../../../pizza2400/.vercel/project.json", "utf8"),
);
if (hosting.projectName !== "pizza2400")
  throw Error("Unexpected hosting project");
const health = await fetch("https://pizza2400.webdock.dev/");
if (!health.ok || !(await health.text()).includes("DEIN ABEND."))
  throw Error("Verify the deployed storefront first");
await runOperator(async (actor) => {
  const p = actor.payload;

  const project = await p.findByID({
    collection: "projects",
    id: registry.projectID,
    user: actor.user,
    overrideAccess: false,
  });
  await writeProject(actor, project.id, {
    name: project.name,
    customer: registry.customerID,
    url: "https://pizza2400.webdock.dev",
    repositoryURL: "https://github.com/spitzli/pizza2400",
    notes: project.notes || "",
  });
  const found = (
    await p.find({
      collection: "cms-instances",
      where: { project: { equals: registry.projectID } },
      overrideAccess: false,
      user: actor.user,
      limit: 1,
    })
  ).docs[0];
  const instance = await writeInstance(
    actor,
    found?.id || null,
    {
      project: registry.projectID,
      label: "Pizza2400 CMS",
      adminURL: managementURL,
      schemaName: "pizza2400",
      providerProjectID: hosting.projectId,
      provider: "vercel",
      template: "custom",
      payloadVersion: "4.0.0-canary.37",
      status: "active",
      notes:
        "Independent Payload instance with Webdock SSO, dedicated schema/runtime role and own public Blob media store. Storefront payments are demo-only.",
    },
    true,
  );
  console.log({
    customerID: registry.customerID,
    projectID: registry.projectID,
    instanceID: instance.id,
  });
});
