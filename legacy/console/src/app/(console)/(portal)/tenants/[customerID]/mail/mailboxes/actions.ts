"use server";
import { headers } from "next/headers";
import { revalidatePath } from "next/cache";
import { manageTenantMailboxes } from "@/lib/mailboxes";
import { StudioError } from "@/lib/studio-errors";
export async function saveMailbox(_previous: { error?: string; message?: string }, data: FormData): Promise<{ error?: string; message?: string }> {
  const text = (name: string) => typeof data.get(name) === "string" ? String(data.get(name)) : "";
  try {
    const input: Record<string,string> = { action: text("action") };
    for (const field of ["senderDomainID","domainID","localPart","name","mailboxID","subject","revision"]) if (data.has(field)) input[field] = text(field);
    if (input.action === "mailbox.create") {
      const size = text("quotaMiB");
      if (!/^[1-9][0-9]{0,3}$/.test(size)) return { error: "Invalid mailbox size." };
      input.quotaBytes = String(Number(size) * 1048576);
    }
    const result = await manageTenantMailboxes(await headers(), text("customer"), input);
    revalidatePath(`/tenants/${text("customer")}/mail/mailboxes`);
    return result;
  } catch (error) { return { error: error instanceof StudioError ? error.message : "Mailbox settings could not be saved." }; }
}
