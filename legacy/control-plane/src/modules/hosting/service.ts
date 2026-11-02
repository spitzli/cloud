import { readProjectOverview } from "./project-overview";
import { readHostingView } from "./view-cache";
import { createHostingService } from "@webdock/module-hosting/server/service";
import { executeProjectSetup } from "./project-setup";
import { executeServiceBinding } from "./service-bindings";
import { relocateProject } from "./relocation";
import { withHostingDatabase } from "../../db/database-context";
import { executeDatabaseCommand } from "./databases";
import { executeGitDeployment } from "./git-deployments";
import { executeByok, byokCluster } from "./byok";
import { executeAppCommand } from "./apps";
import { transaction, audit, planLock, clusterLock } from "./db";
import { authorizeHosting } from "./authorization";
import { clusterView } from "./clusters";
import { replay, recordResult } from "./operations";
export const { executeHosting } = createHostingService({
  readProjectOverview,
  readHostingView,
  get executeProjectSetup() {
    return executeProjectSetup;
  },
  get executeServiceBinding() {
    return executeServiceBinding;
  },
  get relocateProject() {
    return relocateProject;
  },
  get withHostingDatabase() {
    return withHostingDatabase;
  },
  get executeDatabaseCommand() {
    return executeDatabaseCommand;
  },
  get executeGitDeployment() {
    return executeGitDeployment;
  },
  get executeByok() {
    return executeByok;
  },
  get byokCluster() {
    return byokCluster;
  },
  get executeAppCommand() {
    return executeAppCommand;
  },
  get transaction() {
    return transaction;
  },
  get audit() {
    return audit;
  },
  get planLock() {
    return planLock;
  },
  get clusterLock() {
    return clusterLock;
  },
  get authorizeHosting() {
    return authorizeHosting;
  },
  get clusterView() {
    return clusterView;
  },
  get replay() {
    return replay;
  },
  get recordResult() {
    return recordResult;
  },
});
