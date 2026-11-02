/** Transport ownership is independent of product installation. */
export function validateDomainOwnership(domains: readonly string[]) {
  if (domains.length && !domains.includes("auth"))
    throw Error("Private API domains require the Auth foundation");
  const dependencies: Record<string, readonly string[]> = {
    registry: ["portal"],
    integrations: ["registry", "portal"],
    lifecycle: ["registry", "integrations"],
    mcp: ["registry"],
  };
  for (const domain of domains)
    for (const required of dependencies[domain] ?? [])
      if (!domains.includes(required))
        throw Error(`API domain ${domain} requires ${required} ownership`);
}
