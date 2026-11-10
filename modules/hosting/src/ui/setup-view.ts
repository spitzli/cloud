import type {
  SetupConfiguration,
  SetupState,
} from "@spitzli/hosting-contracts";
export function setupStage(state: SetupState) {
  return state === "draft"
    ? "edit"
    : state === "awaiting-confirmation"
      ? "review"
      : "progress";
}
export function canConfirmSetup(input: {
  blockers: string[];
  pending: boolean;
  requiredOfferIDs: string[];
  acceptedOfferIDs: string[];
}) {
  return (
    !input.pending &&
    !input.blockers.length &&
    input.requiredOfferIDs.length === input.acceptedOfferIDs.length &&
    input.requiredOfferIDs.every((id) => input.acceptedOfferIDs.includes(id))
  );
}
export function defaultSetupConfiguration(names: string[]): SetupConfiguration {
  let name = "web",
    index = 2;
  while (names.includes(name)) name = "web-" + index++;
  return {
    name,
    source: {
      kind: "git",
      branch: "main",
      rootDirectory: ".",
      dockerfile: "Dockerfile",
      buildProvider: "isolated",
      workflowPath: ".github/workflows/webdock.yml",
      artifactPrefix: "webdock",
    },
    spec: {
      args: [],
      port: 3000,
      healthPath: "/",
      cpuMillicores: 250,
      memoryBytes: 536870912,
      ephemeralBytes: 268435456,
      replicas: 1,
    },
    database: { mode: "none" },
    mail: null,
    autoPublish: false,
  };
}
