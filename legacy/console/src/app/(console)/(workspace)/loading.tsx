
import { getRequestI18n } from '@webdock/i18n/next';
export default async function Loading() {
  const i18n = await getRequestI18n();

  return (
    <div className="loading-view" role="status">
      <div className="loading-line" />
      <div className="loading-line short" />
      <p>{i18n.t("Loading workspace…")}</p>
    </div>
  );
}
