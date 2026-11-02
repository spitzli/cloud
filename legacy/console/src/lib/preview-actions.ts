"use server";
import { redirect } from "next/navigation";
import { studioCall } from "./studio-client";
export async function startBrowserPreview(form: FormData) {
  const id = form.get("customerID");
  if (typeof id !== "string" || !/^[1-9][0-9]{0,18}$/.test(id))
    throw Error("Invalid customer");
  await studioCall("startTenantPreview", [id]);
  redirect("/tenants/" + id);
}
export async function exitBrowserPreview() {
  const result = await studioCall<{ customerID?: string }>("exitTenantPreview");
  redirect(
    result.customerID && /^[1-9][0-9]{0,18}$/.test(result.customerID)
      ? "/tenants/" + result.customerID
      : "/tenants",
  );
}
