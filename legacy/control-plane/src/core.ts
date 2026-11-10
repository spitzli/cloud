import "reflect-metadata";
import { startServer } from "./bootstrap";
const domains = (process.env.SPITZLI_API_DOMAINS ?? "auth,portal,registry").split(",").filter(Boolean);
if (domains.some(domain => !["auth", "portal", "registry", "mcp"].includes(domain)))
  throw Error("Core distribution only supports auth, portal, registry and MCP domains");
await startServer();
