import { HostingError } from "@spitzli/hosting-contracts";
export function inspectRepositoryFiles(files: Record<string, string>) {
  const entries = Object.entries(files);
  if (
    entries.length > 10 ||
    entries.some(([, text]) => Buffer.byteLength(text) > 65536) ||
    entries.reduce((size, [, text]) => size + Buffer.byteLength(text), 0) >
      262144
  )
    throw new HostingError(
      400,
      "Repository inspection exceeded its size limit.",
    );
  const suggestedServices: string[] = [],
    warnings: string[] = [];
  const dockerfile = Object.hasOwn(files, "Dockerfile") ? "Dockerfile" : null;
  const expose = (files.Dockerfile ?? "")
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => /^EXPOSE\s/i.test(line));
  const ports = expose.flatMap((line) => {
    const match = /^EXPOSE\s+([0-9]+)(?:\/tcp)?\s*(?:#.*)?$/i.exec(line);
    return match ? [Number(match[1])] : [];
  });
  const port =
    expose.length === 1 &&
    ports.length === 1 &&
    ports[0] >= 1024 &&
    ports[0] <= 65535
      ? ports[0]
      : null;
  if (!dockerfile)
    warnings.push(
      "Add a Dockerfile in this directory before deploying from Git.",
    );
  if (files["package.json"])
    try {
      const pkg = JSON.parse(files["package.json"]);
      const dependencies = { ...pkg.dependencies, ...pkg.devDependencies };
      if (
        ["pg", "postgres", "@prisma/client"].some((name) =>
          Object.hasOwn(dependencies, name),
        )
      )
        suggestedServices.push("postgres");
      if (
        ["nodemailer", "resend"].some((name) =>
          Object.hasOwn(dependencies, name),
        )
      )
        suggestedServices.push("mail");
    } catch {
      warnings.push(
        "Package metadata could not be read. Configure the application manually.",
      );
    }
  return { dockerfile, port, suggestedServices, warnings };
}
