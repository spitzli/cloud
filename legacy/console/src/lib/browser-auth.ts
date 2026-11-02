import "server-only";
import { integrationCookies } from "@webdock/api-client";
import { headers } from "next/headers";
import { browserAuthConfiguration } from "./browser-auth-config";
export async function browserRequestCredentials(): Promise<{
  cookie: string;
  bearerToken?: string;
}> {
  if (browserAuthConfiguration.provider === "better-auth")
    return { cookie: (await headers()).get("cookie") ?? "" };
  const { auth } = await import("@clerk/nextjs/server");
  const token = await (await auth()).getToken();
  return {
    cookie: integrationCookies((await headers()).get("cookie") ?? ""),
    ...(token ? { bearerToken: token } : {}),
  };
}
