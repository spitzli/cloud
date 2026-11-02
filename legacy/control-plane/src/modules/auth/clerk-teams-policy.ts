import { ConsoleAuthError } from "./clerk-identity";
export function invitationRole(role?: string) {
  if (role === undefined || role === "" || role === "member")
    return "org:member";
  if (role === "admin") return "org:admin";
  throw new ConsoleAuthError(400, "Choose administrator or member.");
}
export function validateMemberChange(
  members: Array<{ id: string; role: string }>,
  id: string,
  action: string,
  role?: string,
) {
  const target = members.find((m) => m.id === id);
  if (!target) throw new ConsoleAuthError(404, "Team member unavailable.");
  if (action === "role") invitationRole(role);
  if (!["role", "remove"].includes(action))
    throw new ConsoleAuthError(400, "Unknown team action.");
  // Clerk owns membership constraints; local suspension only governs Webdock access.
  return target;
}
