import { getRequestI18n } from '@webdock/i18n/next';
import { redirect } from 'next/navigation';
import { getStudioSession } from '@/lib/studio-client';
import { CodeExample } from '@/components/code-example';

export default async function Developer() {
  if (!(await getStudioSession())) redirect('/api/sso/login?returnTo=%2Fhosting%2Fdeveloper');
  const { t } = await getRequestI18n();
  const origin = process.env.WEBDOCK_STUDIO_ORIGIN || process.env.NEXT_PUBLIC_SERVER_URL || 'https://studio.webdock.dev';
  return <div className="access-page">
    <header className="account-heading"><div><h1>{t('Developer tools')}</h1><p>{t('Connect your tools, inspect applications and follow deployment progress.')}</p></div></header>
    <section className="panel">
      <h2>{t('Connect an MCP client')}</h2>
      <ol className="developer-steps">
        <li>{t('Add this server URL to an MCP client that supports HTTP and OAuth.')}</li>
        <li>{t('Sign in with your Webdock account. For hosting, request hosting:read; add hosting:write only when the client needs to make changes.')}</li>
        <li>{t('Your current workspace permissions also apply to every tool call.')}</li>
      </ol>
      <CodeExample label={t('MCP server URL')} code={origin + "/api/mcp"} />
      <p>{t('Mail sending credentials are separate from Webdock OAuth access tokens.')}</p>
    </section>
    <section className="panel">
      <h2>{t('Hosting API')}</h2>
      <p>{t('Use a Webdock OAuth access token with hosting:read and the MCP resource as its audience. Replace APP_ID with the application ID shown on its overview.')}</p>
      <CodeExample label={t('Inspect an application')} code={`curl --fail-with-body '${origin}/api/hosting/apps/APP_ID' \\\n  -H "Authorization: Bearer $WEBDOCK_ACCESS_TOKEN"`} />
      <p>{t('Find the current operation ID in the application response. Follow it until the requested change is confirmed.')}</p>
      <CodeExample label={t('Inspect an operation')} code={`curl --fail-with-body '${origin}/api/hosting/operations/OPERATION_ID' \\\n  -H "Authorization: Bearer $WEBDOCK_ACCESS_TOKEN"`} />
      <dl>
        <dt>202</dt><dd>{t('Accepted and queued. The change has not finished yet.')}</dd>
        <dt>401 / 403</dt><dd>{t('Check the token, its scopes and your workspace permissions.')}</dd>
        <dt>409</dt><dd>{t('The resource changed or an operation is still running. Read its current state before retrying.')}</dd>
      </dl>
      <p>{t('Changes require hosting:write. Application mutations also use an idempotency key; reuse it when retrying the same request after a network failure.')}</p>
    </section>
  </div>;
}
