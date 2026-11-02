import { readFile } from "node:fs/promises";
import { Pool } from "pg";
import { verifiedPostgresConfig } from "@webdock/database-runtime";
import { readAuthProviderConfiguration } from "@webdock/auth-configuration";
import { enrollClerkIdentity } from "../src/modules/auth/clerk-mappings";
const [file, mode] = process.argv.slice(2);
if (!file || (mode !== undefined && mode !== "--apply"))
  throw Error("Usage: bun scripts/clerk-map.ts manifest.json [--apply]");
const config = readAuthProviderConfiguration(process.env.WEBDOCK_AUTH_PROVIDER);
if (config.provider !== "clerk")
  throw Error("Configure the intended Clerk issuer before mapping identities");
const value: unknown = JSON.parse(await readFile(file, "utf8"));
if (!value || typeof value !== "object" || Array.isArray(value))
  throw Error("Invalid mapping manifest");
const entry = value as Record<string, unknown>;
if (
  Object.keys(entry).sort().join(",") !== "issuer,localSubject,subject" ||
  entry.issuer !== config.issuer ||
  typeof entry.subject !== "string" ||
  !/^user_[A-Za-z0-9]+$/.test(entry.subject) ||
  typeof entry.localSubject !== "string" ||
  !/^[1-9][0-9]{0,18}$/.test(entry.localSubject)
)
  throw Error(
    "Mapping requires the configured issuer, explicit Clerk subject and existing local ID",
  );
const mapping = {
  issuer: config.issuer,
  subject: entry.subject,
  localSubject: entry.localSubject,
};
if (mode !== "--apply") {
  console.log(
    JSON.stringify({
      action: "enroll-existing-identity",
      ...mapping,
      apply: false,
    }),
  );
} else {
  const url = process.env.AUTH_DATABASE_URL;
  if (!url) throw Error("AUTH_DATABASE_URL is required for offline mapping");
  const database = new Pool({
    ...verifiedPostgresConfig(url, process.env.DATABASE_CA_CERT),
    max: 1,
  });
  try {
    await enrollClerkIdentity(database, mapping);
    console.log(JSON.stringify({ action: "identity-enrolled", ...mapping }));
  } finally {
    await database.end();
  }
}
