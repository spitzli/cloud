import {
  createRegistry,
  RegistryError,
  type RegistryIdentity,
} from "./service";
const actors = new WeakMap<Request, RegistryIdentity>();
let registry: ReturnType<typeof createRegistry> | undefined;
export const registryService = () => (registry ??= createRegistry());
export const closeRegistry = async () => {
  if (registry) await (await registry).close();
  registry = undefined;
};
export function setRegistryIdentity(
  request: Request,
  identity: RegistryIdentity,
) {
  actors.set(request, identity);
}
export function registryIdentity(request: Request) {
  const actor = actors.get(request);
  if (!actor) throw new RegistryError("Sign in to Studio.", 401);
  return actor;
}
