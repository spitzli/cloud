
import { getRequestI18n } from '@webdock/i18n/next';
export default async function PortalLoading() {
  const i18n = await getRequestI18n();

  return (
    <div
      className="loading-view"
      role="status"
      aria-label={i18n.t("Loading your workspace")}
    >
      <div className="loading-line" aria-hidden="true" />
      <div className="loading-line short" aria-hidden="true" />
      <p>{i18n.t("Loading your workspace…")}</p>
      <div className="portal-loading-grid" aria-hidden="true">
        <div />
        <div />
      </div>
    </div>
  );
}
