export type GitHubInstallation = { id: number; account: string };
export type GitHubRepository = {
  id: number;
  name: string;
  url: string;
  private: boolean;
};
export type GitHubView = {
  configured: boolean;
  session: { login: string } | null;
  installURL: string | null;
  permissionsURL: string | null;
  callbackURL: string;
  installations: GitHubInstallation[];
  repositories: GitHubRepository[];
  installationPages: number;
  repositoryPages: number;
  installation: number;
  installationPage: number;
  page: number;
  problem: boolean;
};
export type VercelView = {
  configured: boolean;
  connected: boolean;
  problem: boolean;
  available: {
    projects: { id: string; name: string }[];
    nextCursor: string | null;
  };
};
export type VercelProjectView =
  | {
      state: "ready";
      snapshot: import("@spitzli/hosting-contracts/vercel").VercelProjectSnapshot;
    }
  | {
      state:
        | "setup"
        | "unlinked"
        | import("@spitzli/hosting-contracts/vercel").VercelAPIErrorCode;
    };
