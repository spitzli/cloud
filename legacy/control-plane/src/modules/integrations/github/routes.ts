import { RegistryError } from "../../registry/service";
import { currentRegistryActor } from "../operator";
import { getGitHub, githubOrigin, githubReturnPath } from "./client";
import { linkGitHubRepository } from "./project";

const notice = (status: string, returnTo = "/integrations") =>
  new Response(null, {
    status: 303,
    headers: {
      Location: new URL(`${returnTo}?github=${status}`, githubOrigin()).href,
      "Cache-Control": "no-store",
      "Referrer-Policy": "no-referrer",
    },
  });
function returnTo(request: Request) {
  const query = new URL(request.url).searchParams;
  const path = query.get("returnTo") ?? "/integrations";
  return query.getAll("returnTo").length <= 1 && githubReturnPath(path)
    ? path
    : new Response(null, {
        status: 400,
        headers: { "cache-control": "no-store" },
      });
}
async function operator(_request: Request) {
  try {
    return await currentRegistryActor();
  } catch (error) {
    return new Response(null, {
      status: error instanceof RegistryError ? error.status : 503,
    });
  }
}

export async function githubConnect(request: Request) {
  const target = returnTo(request);
  if (target instanceof Response) return target;
  const actor = await operator(request);
  if (actor instanceof Response) return actor;
  if (!actor) return new Response(null, { status: 403 });
  const github = getGitHub();
  return github
    ? github.connect(request, actor.user.id, target)
    : notice("setup", target);
}
export async function githubCallback(request: Request) {
  const actor = await operator(request);
  if (actor instanceof Response) return actor;
  const github = getGitHub();
  if (!github) return notice("setup");
  return github.callback(request, actor?.user.id || "");
}
export async function githubDisconnect(request: Request) {
  const target = returnTo(request);
  if (target instanceof Response) return target;
  const actor = await operator(request);
  if (actor instanceof Response) return actor;
  if (!actor) return new Response(null, { status: 403 });
  const github = getGitHub();
  return github ? github.disconnect(request, target) : notice("setup", target);
}
export async function githubSelect(request: Request) {
  if (
    request.method !== "POST" ||
    request.headers.get("origin") !== new URL(githubOrigin()).origin
  )
    return new Response(null, { status: 403 });
  const actor = await operator(request);
  if (actor instanceof Response) return actor;
  if (!actor) return new Response(null, { status: 403 });
  if (
    !request.headers
      .get("content-type")
      ?.startsWith("application/x-www-form-urlencoded")
  )
    return new Response(null, { status: 415 });
  if (Number(request.headers.get("content-length")) > 4096)
    return new Response(null, { status: 413 });
  const reader = request.body?.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  if (reader)
    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        size += value.byteLength;
        if (size > 4096) {
          await reader.cancel();
          return new Response(null, { status: 413 });
        }
        chunks.push(value);
      }
    } finally {
      reader.releaseLock();
    }
  const body = Buffer.concat(chunks).toString("utf8");
  const github = getGitHub();
  if (!github) return notice("setup");
  const session = await github.session(request.headers, actor.user.id);
  if (!session) return notice("expired");
  try {
    const form = new URLSearchParams(body);
    if ([...form.keys()].some((key) => form.getAll(key).length !== 1))
      return notice("save-failed");
    const project = await linkGitHubRepository(
      actor,
      github,
      session,
      Object.fromEntries(form),
    );

    return new Response(null, {
      status: 303,
      headers: {
        Location: new URL(`/projects/${project.id}`, githubOrigin()).href,
        "Cache-Control": "no-store",
      },
    });
  } catch {
    return notice("save-failed");
  }
}
