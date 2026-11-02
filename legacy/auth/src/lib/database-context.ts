/** Each HTTP operation authenticates and scopes its own API transaction. */
export async function withAuthenticatedDatabase<T>(_headers:Headers,run:()=>Promise<T>):Promise<T>{return run();}
