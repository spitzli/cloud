import "server-only";
import { getStudioSession } from "./studio-client";
// Portal pages consume the same safe shape; the identity cookie stays on Auth.
export const auth = { api: { getSession: async (input: { headers: Headers }) => { void input; return getStudioSession(); } } };
