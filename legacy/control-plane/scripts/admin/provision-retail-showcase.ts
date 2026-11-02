import fs from "node:fs";
import path from "node:path";
import { runOperator, requireApply } from "./context";
import {
  writeCustomer,
  writeProject,
  writeInstance,
} from "../../src/modules/registry/compat";
requireApply();
const root = path.resolve(process.cwd(), "../../..");
await runOperator(async (actor) => {
  const payload = actor.payload;
  const { database: db } = await import("../../src/db/runtime");
  const out = path.join(root, "webdock-demos/retail-showcase-registry.json");
  const reg = fs.existsSync(out)
    ? JSON.parse(fs.readFileSync(out, "utf8"))
    : {};
  const id = "WD-SHOWCASE-RETAIL-NORDLICHT",
    entry = reg[id] || {};
  const save = () =>
    fs.writeFileSync(
      out,
      JSON.stringify({ ...reg, [id]: entry }, null, 2) + "\n",
    );

  if (!entry.customerID) {
    const found = await payload.find({
      collection: "customers",
      where: { name: { equals: "Nordlicht Markt · fiktiver Einzelhandel" } },
      overrideAccess: true,
    });
    if (found.totalDocs)
      throw Error("Existing showcase customer without local receipt");
    const c = await writeCustomer(actor, null, {
      name: "Nordlicht Markt · fiktiver Einzelhandel",
      customerType: "company",
      companyName: "Nordlicht Markt – fiktiver Musterladen",
      contactEmail: "nordlicht-markt@example.invalid",
      country: "DE",
      notes:
        "Fiktiver deutschsprachiger Webdock-Testmandant. Keine echten Kundendaten, keine Einladungen, kein Mailversand und keine Zahlungen. Separater Showcase außerhalb der 15 Lead-Demos.",
    });
    entry.customerID = c.id;
    save();
  }
  entry.tenantID = (
    await db.query(
      "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
      [entry.customerID],
    )
  ).rows[0]?.organization_id;
  if (!entry.tenantID) throw Error("Tenant trigger did not run");
  save();
  if (!entry.projectID) {
    const p = await writeProject(actor, null, {
      name: "Nordlicht Markt – deutscher Demo-Shop",
      customer: entry.customerID,
      url: "https://nordlicht-markt.webdock.dev",
      notes:
        "Fiktiver Einzelhandel mit Produkten, Varianten, Extras, Warenkorb und ausschließlich simulierten Bestellungen. Eigenständiger Showcase, keine Lead-Vertriebszuordnung.",
    });
    entry.projectID = p.id;
    save();
  }
  if (process.argv.includes("--binding") && !entry.instanceID) {
    const host = JSON.parse(
      fs.readFileSync(
        path.join(
          root,
          "webdock-demos/sites/nordlicht-markt/.vercel/project.json",
        ),
        "utf8",
      ),
    );
    const b = await writeInstance(
      actor,
      null,
      {
        project: entry.projectID,
        label: "Nordlicht Markt · Shop & Produktverwaltung",
        adminURL: "https://nordlicht-markt.webdock.dev/admin",
        schemaName: "nordlicht_markt",
        providerProjectID: host.projectId,
        provider: "vercel",
        template: "custom",
        payloadVersion: "4.0.0-canary.37",
        status: "active",
        notes:
          "Fiktiver Testbetrieb: deutscher Demo-Shop, Produktverwaltung, Medien, Varianten, Extras und simulierte Bestellungen. Kein Versand, keine Bezahlung.",
      },
      true,
    );
    entry.instanceID = b.id;
    entry.url = "https://nordlicht-markt.webdock.dev";
    entry.vercelProjectID = host.projectId;
    save();
  }
  if (process.argv.includes("--link-cms"))
    await writeInstance(
      actor,
      entry.instanceID,
      {
        project: entry.projectID,
        label: "Nordlicht Markt · Shop & Produktverwaltung",
        adminURL: entry.cmsURL,
        schemaName: "nordlicht_markt",
        providerProjectID: entry.vercelProjectID,
        provider: "vercel",
        template: "custom",
        payloadVersion: "4.0.0-canary.37",
        status: "active",
        notes:
          "Fiktiver Testbetrieb: deutscher Demo-Shop, Produktverwaltung, Medien, Varianten, Extras und simulierte Bestellungen. Kein Versand, keine Bezahlung.",
      },
      true,
    );
  console.log(JSON.stringify(entry));
});
