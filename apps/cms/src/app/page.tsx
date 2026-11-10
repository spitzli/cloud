import {getRequestI18n} from '@spitzli/i18n/next';
export default async function Page() {
  const {t}=await getRequestI18n();
  return <main style={{ maxWidth: '42rem', margin: '12vh auto', padding: '2rem', lineHeight: 1.7 }}>
    <p>{"WEBDOCK"}</p>
    <h1>{t("Your website has its own admin.")}</h1>
    <p>{t("The shared CMS has been retired. Sign in to your website to manage its content.")}</p>
    <ul>
      <li><a href="https://webdock.dev/admin">{t("Webdock admin ↗")}</a></li>
      <li><a href="https://spitzli.vercel.app/admin">{t("Spitzli Development admin ↗")}</a></li>
      <li><a href="https://www.stall-eichenbruch.de/admin">{t("Stall Eichenbruch admin ↗")}</a></li>
    </ul>
    <p>{t("Your existing email address and password still work.")}</p>
  </main>;
}
