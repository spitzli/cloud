import Image from 'next/image';
import {getRequestI18n} from '@webdock/i18n/next';
export async function Dock() {
  const {t}=await getRequestI18n();
  return <figure className="dock" aria-label={t("Illustration: a workspace, an application and its domain connected through Spitzli Cloud.")}>
    <div className="dock-top"><span>{t("Your projects. Connected.")}</span><span className="dock-coordinate">{"spitzli.cloud"}</span></div>
    <svg className="dock-lines" viewBox="0 0 640 540" fill="none" aria-hidden="true"><path d="M155 134H260Q285 134 285 159V280M492 212H390Q365 212 365 237V280M320 325V397Q320 425 348 425H463"/><circle cx="285" cy="205" r="5"/><circle cx="365" cy="254" r="5"/><circle cx="380" cy="425" r="5"/></svg>
    <div className="project-node preview-node"><span className="node-symbol">▧</span><span>{t("Your workspace")}<small>{"console.spitzli.cloud"}</small></span></div>
    <div className="project-node website-node"><span className="node-symbol">↗</span><span>{t("Your application")}<small>{"project.spitzli.site"}</small></span></div>
    <div className="dock-hub"><Image src="/spitzli-logo.png" alt="" width={24} height={24}/><span>{"Spitzli Cloud"}</span></div>
    <div className="domain-node"><span className="domain-dot"/>{"your-domain.com"}<span>↗</span></div>
    <figcaption>{t("Example addresses. Your project gets a place of its own.")}</figcaption>
  </figure>;
}
