export function vercelSettings() {
  const clientID = process.env.SPITZLI_VERCEL_CLIENT_ID,
    clientSecret = process.env.SPITZLI_VERCEL_CLIENT_SECRET,
    slug = process.env.SPITZLI_VERCEL_INTEGRATION_SLUG,
    teamID = process.env.SPITZLI_VERCEL_TEAM_ID,
    teamSlug = process.env.SPITZLI_VERCEL_TEAM_SLUG,
    cookieSecret = process.env.SPITZLI_SSO_COOKIE_SECRET;
  if (
    !clientID ||
    !clientSecret ||
    !slug ||
    !teamID ||
    !teamSlug ||
    !cookieSecret
  )
    return null;
  return {
    clientID,
    clientSecret,
    slug,
    teamID,
    teamSlug,
    cookieSecret,
    origin: process.env.SPITZLI_STUDIO_ORIGIN || "http://localhost:3120",
  };
}
