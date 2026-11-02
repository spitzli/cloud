import { runOperator, requireApply } from "./context";
import fs from "node:fs";
import path from "node:path";
const root = path.resolve("../../../webdock-demos"),
  sites = [
    ...JSON.parse(fs.readFileSync(path.join(root, "manifest.json"), "utf8")),
    {
      id: "WD-PIZZA2400",
      name: "Pizza2400",
      slug: "pizza2400",
      domain: "pizza2400.webdock.dev",
    },
  ],
  registry = JSON.parse(
    fs.readFileSync(path.join(root, "registry.json"), "utf8"),
  );
const report: {
  leadID: string;
  name: string;
  projectID: string;
  customerID: string;
  instanceID?: string;
  adminURL?: string;
  demoURL?: string | null;
  linked: boolean;
}[] = [];
await runOperator(async (actor) => {
  const p = actor.payload;
  const user = actor.user;
  for (const site of sites) {
    const ids = registry[site.id];
    const project = await p.findByID({
      collection: "projects",
      id: ids.projectID,
      overrideAccess: false,
      user,
      depth: 0,
    });
    const instances = (
      await p.find({
        collection: "cms-instances",
        where: { project: { equals: ids.projectID } },
        overrideAccess: false,
        user,
        depth: 0,
        limit: 2,
      })
    ).docs;
    const instance = instances[0];
    const expected = "https://studio.webdock.dev/sites/" + ids.bindingID;
    const ok =
      instances.length === 1 &&
      instance.id === ids.instanceID &&
      instance.adminURL === expected &&
      instance.providerProjectID === ids.vercelProjectID &&
      instance.schemaName ===
        (site.id === "WD-PIZZA2400"
          ? "pizza2400"
          : "demo_" + site.slug.replaceAll("-", "_")) &&
      project.customer === ids.customerID &&
      project.url === "https://" + site.domain;
    report.push({
      leadID: site.id,
      name: site.name,
      projectID: ids.projectID,
      customerID: ids.customerID,
      instanceID: instance?.id,
      adminURL: instance?.adminURL,
      demoURL: project.url,
      linked: ok,
    });
    console.log(site.id + (ok ? " LINKED_OK" : " MISMATCH"));
  }
  fs.writeFileSync(
    path.join(root, "cms-audit.json"),
    JSON.stringify(report, null, 2) + "\n",
  );
  if (report.some((r) => !r.linked))
    throw Error("One or more projects lack a verified CMS link");
});
