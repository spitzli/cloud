import {getRequestI18n} from '@spitzli/i18n/next';
export async function SSOLogin() {
  const {t}=await getRequestI18n();
  if (!process.env.WEBDOCK_SSO_CLIENT_ID) return null;
  return (
    <form action="/api/sso/login" method="get" style={{ marginBottom: '2rem' }}>
      <button className="btn btn--style-primary" type="submit">{t("Sign in with Spitzli Cloud")}</button>
    </form>
  );
}
