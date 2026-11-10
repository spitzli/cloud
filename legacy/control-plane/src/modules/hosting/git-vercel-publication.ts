import { createHostingGitVercelPublication } from "@spitzli/module-hosting/server/git-vercel-publication";
import { vercelJSON } from "./byok-vercel";
export const { createGitVercelObserver, gitReleaseHealthPath } =
  createHostingGitVercelPublication({
    get vercelJSON() {
      return vercelJSON;
    },
  });
