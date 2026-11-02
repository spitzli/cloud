export function vercelSettings() {
  const clientID = process.env.WEBDOCK_VERCEL_CLIENT_ID,
    clientSecret = process.env.WEBDOCK_VERCEL_CLIENT_SECRET,
    slug = process.env.WEBDOCK_VERCEL_INTEGRATION_SLUG,
    teamID = process.env.WEBDOCK_VERCEL_TEAM_ID,
    teamSlug = process.env.WEBDOCK_VERCEL_TEAM_SLUG,
    cookieSecret = process.env.WEBDOCK_SSO_COOKIE_SECRET;
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
    origin: process.env.WEBDOCK_STUDIO_ORIGIN || "http://localhost:3120",
  };
}
