
import { getRequestI18n } from '@webdock/i18n/next';
import Link from "next/link";
export default async function NotFound() {
  const i18n = await getRequestI18n();

  return (
    <section className="panel empty">
      <h1>{i18n.t("Record not found")}</h1>
      <p>{i18n.t("This link does not match an available customer or project. Search your workspace to find the record.")}</p>
      <Link className="button" href="/">{i18n.t("Back to projects")}</Link>
    </section>
  );
}
