import { guardOperation } from "./operations";
import { templateAgentRequest } from "./plugin-template-runtime";
import { createHostingAgentApi } from "@webdock/module-hosting/server/agent-api";
import { withDatabaseScope } from "@webdock/database-runtime";
import { authDatabase } from "../../db/runtime";
import {
  databaseObservationConfig,
  reportDatabaseObservations,
} from "./database-observations";
import {
  claimDatabaseOperation,
  checkpointDatabaseOperation,
  completeDatabaseOperation,
  guardDatabaseOperation,
} from "./database-agent";
import {
  claimMailConfiguration,
  checkpointMailConfiguration,
  completeMailConfiguration,
} from "../mail/mail-configuration";
import {
  claimNativeMail,
  checkpointNativeMail,
  completeNativeMail,
} from "../mail/mail-agent";
import {
  saveInventory,
  byokAgentConfig,
  claimByokOperation,
  completeByokOperation,
} from "./byok";
import { claimOperation, completeOperation } from "./operations";
import { database } from "../../db/runtime";
import {
  authenticateAgent,
  consumeEnrollment,
  reportCluster,
} from "./clusters";
import { requireHostingEnvironment, readHostingJSON } from "./http";
export const { hostingAgentAPI } = createHostingAgentApi({
  templateAgentRequest,
  guardOperation,
  get withDatabaseScope() {
    return withDatabaseScope;
  },
  get authDatabase() {
    return authDatabase;
  },
  get databaseObservationConfig() {
    return databaseObservationConfig;
  },
  get reportDatabaseObservations() {
    return reportDatabaseObservations;
  },
  get claimDatabaseOperation() {
    return claimDatabaseOperation;
  },
  get checkpointDatabaseOperation() {
    return checkpointDatabaseOperation;
  },
  get completeDatabaseOperation() {
    return completeDatabaseOperation;
  },
  get guardDatabaseOperation() {
    return guardDatabaseOperation;
  },
  get claimMailConfiguration() {
    return claimMailConfiguration;
  },
  get checkpointMailConfiguration() {
    return checkpointMailConfiguration;
  },
  get completeMailConfiguration() {
    return completeMailConfiguration;
  },
  get claimNativeMail() {
    return claimNativeMail;
  },
  get checkpointNativeMail() {
    return checkpointNativeMail;
  },
  get completeNativeMail() {
    return completeNativeMail;
  },
  get saveInventory() {
    return saveInventory;
  },
  get byokAgentConfig() {
    return byokAgentConfig;
  },
  get claimByokOperation() {
    return claimByokOperation;
  },
  get completeByokOperation() {
    return completeByokOperation;
  },
  get claimOperation() {
    return claimOperation;
  },
  get completeOperation() {
    return completeOperation;
  },
  get database() {
    return database;
  },
  get authenticateAgent() {
    return authenticateAgent;
  },
  get consumeEnrollment() {
    return consumeEnrollment;
  },
  get reportCluster() {
    return reportCluster;
  },
  get requireHostingEnvironment() {
    return requireHostingEnvironment;
  },
  get readHostingJSON() {
    return readHostingJSON;
  },
});
