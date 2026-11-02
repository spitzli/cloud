import { runOperator, requireApply } from "./context";
// Offline operator tool. No invitations or emails. Dry-run unless --apply.
import fs from "node:fs";
import path from "node:path";
import { writeCustomer, writeProject } from "../../src/modules/registry/compat";
const root = path.resolve("../../../webdock-demos"),
  file = path.join(root, "registry.json");
const manifest = JSON.parse(
  fs.readFileSync(path.join(root, "manifest.json"), "utf8"),
);
const registry: Record<string, { customerID: string; projectID?: string }> =
  fs.existsSync(file) ? JSON.parse(fs.readFileSync(file, "utf8")) : {};
const apply = process.argv.includes("--apply");
const save = () => {
  fs.writeFileSync(file + ".tmp", JSON.stringify(registry, null, 2) + "\n");
  fs.renameSync(file + ".tmp", file);
};
await runOperator(async (actor) => {
  const p = actor.payload;

  for (const item of manifest) {
    const lead = JSON.parse(
        fs.readFileSync(
          path.join(root, "sites", item.slug, "lead.json"),
          "utf8",
        ),
      ),
      marker = "Webdock Lead: " + item.id;
    const candidates = (
      await p.find({
        collection: "customers",
        where: {
          or: [
            { notes: { contains: marker } },
            { name: { equals: lead.name } },
          ],
        },
        overrideAccess: false,
        user: actor.user,
        limit: 100,
      })
    ).docs;
    if (candidates.length > 1) throw Error("Ambiguous customer " + item.id);
    let customer = registry[item.id]?.customerID
      ? await p.findByID({
          collection: "customers",
          id: registry[item.id].customerID,
          overrideAccess: false,
          user: actor.user,
        })
      : candidates[0];
    if (customer && customer.name !== lead.name)
      throw Error("Customer conflict " + item.id);
    if (!customer && apply) {
      const address = lead.address.match(
        /^(.+?),\s*(\d{5})\s+(.+?)(?:\s*\(|$)/,
      );
      customer = await writeCustomer(actor, null, {
        name: lead.name,
        customerType: "company",
        companyName: lead.name,
        contactName: lead.contactPerson || undefined,
        contactEmail: lead.email || undefined,
        phone: lead.phone || undefined,
        addressLine1: address?.[1],
        postalCode: address?.[2],
        city: lead.town,
        country: "DE",
        notes:
          marker +
          " · Prospect demo requested by Dominik, 2026-10-05. Not a signed customer engagement. No invitation or mail sent. Source: " +
          lead.website,
      });
      registry[item.id] = { customerID: customer.id };
      save();
    }
    if (!customer) {
      console.log(item.id + " would create customer and project");
      continue;
    }
    registry[item.id] ??= { customerID: customer.id };
    const matches = (
      await p.find({
        collection: "projects",
        where: {
          and: [
            { customer: { equals: customer.id } },
            {
              or: [
                { notes: { contains: marker } },
                { url: { equals: "https://" + item.domain } },
              ],
            },
          ],
        },
        overrideAccess: false,
        user: actor.user,
        limit: 100,
      })
    ).docs;
    if (matches.length > 1) throw Error("Ambiguous project " + item.id);
    let project = registry[item.id].projectID
      ? await p.findByID({
          collection: "projects",
          id: registry[item.id].projectID!,
          overrideAccess: false,
          user: actor.user,
        })
      : matches[0];
    if (
      project &&
      (typeof project.customer === "object"
        ? project.customer.id
        : project.customer) !== customer.id
    )
      throw Error("Project owner conflict");
    if (!project && apply)
      project = await writeProject(actor, null, {
        name: lead.name + " · Website-Demo",
        customer: customer.id,
        url: "https://" + item.domain,
        notes:
          marker +
          " · Independent concept demo: modern / and existing-style /old. Separate Payload test inbox. No fulfillment, payment or business mail.",
      });
    if (project) {
      registry[item.id].projectID = project.id;
      if (apply) save();
    }
    console.log(item.id, apply ? "registered" : "existing", {
      customerID: customer.id,
      projectID: project?.id,
    });
  }
});
