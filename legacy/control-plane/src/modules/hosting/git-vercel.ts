import { createHostingGitVercel } from "@spitzli/module-hosting/server/git-vercel";
import { decryptMailSecret } from "../platform/platform";
import { vercelJSON } from "./byok-vercel";
export { gitVercelSchemaSQL } from "@spitzli/module-hosting/server/git-vercel";
export type { GitVercelCredential } from "@spitzli/module-hosting/server/git-vercel";
export const { resolveGitVercelCredential, prepareGitVercelBuild } =
  createHostingGitVercel({
    get decryptMailSecret() {
      return decryptMailSecret;
    },
    get vercelJSON() {
      return vercelJSON;
    },
    env: process.env,
  });
