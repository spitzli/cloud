import { createGitHubProvider as createProvider } from "@spitzli/adapter-hosting-github";
import type { GitHubConfig } from "@spitzli/module-hosting/github";
import type { Fetch } from "../shared/http";
export { parseGitHubWebhook } from "@spitzli/adapter-hosting-github";
export type {
  GitHubConfig,
  GitRepositoryBinding,
  GitSource,
  GitHubWebhook,
} from "@spitzli/module-hosting/github";
export function createGitHubProvider(
  config: GitHubConfig = {
    appID: process.env.SPITZLI_GITHUB_APP_ID ?? "",
    privateKey:
      process.env.SPITZLI_GITHUB_APP_PRIVATE_KEY ||
      process.env.SPITZLI_GITHUB_PRIVATE_KEY ||
      "",
    clientID: process.env.SPITZLI_GITHUB_CLIENT_ID ?? "",
    clientSecret: process.env.SPITZLI_GITHUB_CLIENT_SECRET ?? "",
  },
  fetcher: Fetch = fetch,
) {
  return createProvider(config, fetcher, {
    get allowLoopback() {
      return process.env.NODE_ENV !== "production";
    },
  });
}
