// The repository root, for files tests read or write that are not modules (fixtures, screenshots).
const root = new URL('../../../', import.meta.url);
export const fromRepo = (path) => new URL(path, root);
