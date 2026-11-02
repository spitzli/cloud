// Only explicit local destinations can resume after password/passkey/2FA login.
export function accountContinuation(search: string): string | null {
  const params = new URLSearchParams(search);
  if (params.has("sig")) return null;
  const invitation = params.get("invitation");
  if (invitation && /^[1-9][0-9]{0,18}$/.test(invitation)) return `/invitation?id=${invitation}`;
  const offer = params.get("offer");
  if (offer && /^[A-Za-z0-9_-]{43}$/.test(offer)) return `/offers/${offer}`;
  return params.get("returnTo") === "sites" ? "/sites" : null;
}
