import { registryStore, type RegistryStore } from "../registry/store";
import { delegatedToken } from "../portal/request-context";
import { currentDatabaseScope } from "@webdock/database-runtime";
import { registryService } from "../registry/context";
import type { RegistryIdentity } from "../registry/service";
import { RegistryError } from "../registry/service";
export type RegistryActor = {
  payload: RegistryStore;
  accessToken?: string;
  identity: RegistryIdentity;
  user: {
    id: string;
    name: string;
    email: string;
    role: string;
    collection: "users";
  };
};
export async function currentRegistryActor(): Promise<RegistryActor> {
  const scope = currentDatabaseScope();
  if (scope?.kind !== "session")
    throw new RegistryError("Sign in to Studio.", 401);
  const identity = { subject: scope.subject, sessionID: scope.sessionID },
    user = await (await registryService()).operator(identity);
  let accessToken: string | undefined;
  try {
    accessToken = delegatedToken();
  } catch {}
  return {
    identity,
    user,
    payload: await registryStore(identity),
    accessToken,
  };
}
