import { readFile, open } from "node:fs/promises";
import { enrollGitWorker } from "../../src/modules/hosting/git-deployments";
import { database } from "../../src/db/runtime";

// Trusted administrative CLI. The active operator session is revalidated by hosting.
// Enrollment credentials are written once to a private file, never printed.
try {
  const [configPath, outputPath] = process.argv.slice(2);
  if (!configPath || !outputPath)
    throw Error(
      "Usage: enroll-git-worker.ts verified-config.json credential-output",
    );
  const config = JSON.parse(await readFile(configPath, "utf8"));
  const output = await open(outputPath, "wx", 0o600);
  try {
    const result = await enrollGitWorker(
      {
        subject: config.operatorID,
        sessionID: config.sessionID,
        source: "studio",
        scopes: ["hosting:read", "hosting:write"],
      },
      {
        country: config.country,
        isolation: config.isolation ?? "microvm",
        evidence: config.evidence,
        capacity: config.capacity,
      },
    );
    await output.writeFile(result.credential + "\n");
    console.log(
      `Worker ${result.id} enrolled. Credential written to the requested private file.`,
    );
  } finally {
    await output.close();
  }
} finally {
  await database.end();
}
