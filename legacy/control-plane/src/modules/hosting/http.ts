import { createHostingHttp } from "@spitzli/module-hosting/server/http";

export const { requireHostingEnvironment, readHostingJSON } = createHostingHttp(
  { env: process.env },
);
