import { createHostingHttp } from "@webdock/module-hosting/server/http";

export const { requireHostingEnvironment, readHostingJSON } = createHostingHttp(
  { env: process.env },
);
