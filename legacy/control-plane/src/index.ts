import { configureFullProducts } from "./full-products";
import { startServer } from "./bootstrap";
configureFullProducts();
await startServer({ mcp: async config => (await import("./protocols/full-mcp")).mcpProtocol(config), mounts: async config => {
  const mounts = await (await import("./protocols/legacy-auth")).legacyAuthProtocols(config);
  if (config.domains.includes("integrations")) mounts.push(...(await import("./protocols/integrations")).integrationProtocols(config));
  return mounts;
}});
