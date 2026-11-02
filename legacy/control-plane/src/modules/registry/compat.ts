import type {
  Customer,
  Project,
  CmsInstance,
} from "@webdock/api-client/registry";
const serial = <T>(value: unknown): T => JSON.parse(JSON.stringify(value)) as T;
export {
  recordID,
  customerInput,
  projectInput,
  listInput,
  collectionName,
} from "./contracts";
export type { RegistryActor } from "../integrations/operator";
import type { RegistryActor } from "../integrations/operator";
import { registryService } from "./context";
export const listRecords = async (actor: RegistryActor, input: unknown) =>
  (await registryService()).listRecords(actor.identity, input);
export const getRecord = async (
  actor: RegistryActor,
  collection: unknown,
  id: string,
) => (await registryService()).getRecord(actor.identity, collection, id);
export const writeCustomer = async (
  actor: RegistryActor,
  id: string | null,
  input: unknown,
) =>
  serial<Customer>(
    await (await registryService()).writeCustomer(actor.identity, id, input),
  );
export const writeProject = async (
  actor: RegistryActor,
  id: string | null,
  input: unknown,
) =>
  serial<Project>(
    await (await registryService()).writeProject(actor.identity, id, input),
  );
export const writeInstance = async (
  actor: RegistryActor,
  id: string | null,
  input: unknown,
  confirmed: boolean,
) =>
  serial<CmsInstance>(
    await (
      await registryService()
    ).writeInstance(actor.identity, id, input, confirmed),
  );
export const setArchived = async (
  actor: RegistryActor,
  collection: "customers" | "projects",
  id: string,
  archived: boolean,
) =>
  (await registryService()).archive(actor.identity, collection, id, archived);
