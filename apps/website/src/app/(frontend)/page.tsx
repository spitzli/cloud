import { IntentLink } from '@/components/intent-link';
import Image from 'next/image';
import {LanguagePicker} from '@webdock/i18n/picker';
import {getRequestI18n} from '@webdock/i18n/next';
import { ThemePicker } from '@/components/theme-picker';
import { Dock } from '@/components/dock';
import { getLanding } from '@/lib/landing';
import type { Metadata } from 'next';

export const dynamic = 'force-dynamic';
export async function generateMetadata(): Promise<Metadata> {
  const page = await getLanding();
  return { title: page.seoTitle, description: page.seoDescription, openGraph: { title: page.seoTitle, description: page.seoDescription, images: ['/og'] }, twitter: { title: page.seoTitle, description: page.seoDescription, card: 'summary_large_image', images: ['/og'] } };
}

function Mark() { return <Image className="spitzli-mark" src="/spitzli-logo.png" alt="" width={40} height={40}/>; }
export default async function Home() {
  const {t,locale}=await getRequestI18n();
  const page = await getLanding();
  const faqs = page.faqs;
  const waitlist = "https://console.spitzli.cloud/waitlist";
  const contact = `mailto:${page.contactEmail}?subject=${encodeURIComponent(t("My project with Spitzli Cloud"))}`;
  const structuredData = { '@context': 'https://schema.org', '@graph': [
    { '@type': 'Organization', '@id': 'https://spitzli.dev/#organization', name: 'Spitzli Development', url: 'https://spitzli.dev' },
    { '@type': 'WebSite', '@id': 'https://spitzli.cloud/#website', name: 'Spitzli Cloud', url: 'https://spitzli.cloud', inLanguage: locale, description: page.seoDescription, publisher: { '@id': 'https://spitzli.dev/#organization' } },
    ...(faqs.length ? [{ '@type': 'FAQPage', '@id': 'https://spitzli.cloud/#faq', mainEntity: faqs.map(({ question, answer }) => ({ '@type': 'Question', name: question, acceptedAnswer: { '@type': 'Answer', text: answer } })) }] : []),
  ] };
  return <>
    <script type="application/ld+json" dangerouslySetInnerHTML={{ __html: JSON.stringify(structuredData).replace(/</g, '\\u003c') }}/>
    <a className="skip-link" href="#content">{t("Skip to content")}</a>
    <header className="site-header wrap flex items-center justify-between">
      <a href="#" className="brand" aria-label={t("Spitzli Cloud home")}><Mark/>{"Spitzli Cloud"}<span className="brand-period">.</span></a>
      <nav aria-label={t("Main navigation")} className="flex items-center gap-4 sm:gap-8"><a className="nav-section" href="#concept">{t("The idea")}</a>{faqs.length > 0 && <a className="nav-section" href="#faq">{t("FAQ")}</a>}<a href="https://console.spitzli.cloud">{t("Login")}</a><a className="nav-contact" href={waitlist} title={t("Access by invitation only.")}>{t("Waitlist")} <span aria-hidden="true">↗</span></a></nav>
    </header>
    <main id="content">
      <section className="hero wrap">
        <div className="hero-copy"><a className="byline" href="https://spitzli.dev" target="_blank" rel="noopener noreferrer"><span className="small-mark">✳</span> {t("A home for projects by Spitzli")}</a>
          <h1 className="whitespace-pre-line">{page.heroTitle}</h1>
          <p className="hero-description">{page.heroDescription}</p>
          <div className="hero-actions flex flex-wrap items-center gap-6"><a className="button" href={waitlist}>{t("Join the waitlist")} <span aria-hidden="true">↗</span></a><a className="text-link" href={contact}>{page.heroButton} <span aria-hidden="true">↗</span></a></div>
          <p className="hero-note"><span/>{page.heroNote}</p>
          <p className="hero-note">{t("Access by invitation only.")}</p>
        </div>
        <Dock/>
      </section>
      <div className="address-strip"><div className="wrap flex flex-wrap items-center justify-between gap-4"><span>{t("Every idea needs a place.")}</span><p><span>{"your-project"}</span><strong>{".spitzli.site"}</strong><span className="address-cursor" aria-hidden="true"/></p></div></div>
      <section id="concept" className="concept wrap section-space">
        <div><h2 className="whitespace-pre-line">{page.conceptTitle}</h2><p className="section-intro">{page.conceptDescription}</p><a className="text-link" href="https://spitzli.dev" target="_blank" rel="noopener noreferrer">{t("Built by Spitzli")} <span aria-hidden="true">↗</span></a></div>
        <div className="use-cases">{page.useCases.map((item, index) => <article key={item.id}><span className="case-icon" aria-hidden="true">{['▧', '⌁', '↗'][index % 3]}</span><div><h3>{item.title}</h3><p>{item.description}</p></div></article>)}</div>
      </section>
      {faqs.length > 0 && <section id="faq" className="faq wrap section-space"><div><h2 className="whitespace-pre-line">{page.faqTitle}</h2><p className="section-intro">{t("Something else on your mind?")}<br/><a href={contact}>{t("Just ask.")}</a></p></div><div className="faq-list">{faqs.map(({question, answer}) => <details key={question}><summary>{question}<span aria-hidden="true">+</span></summary><p>{answer}</p></details>)}</div></section>}
      <section className="outlook wrap"><span className="outlook-icon" aria-hidden="true">⌘</span><div><h3>{page.outlookTitle}</h3><p>{page.outlookDescription}</p><a className="text-link" href="https://console.spitzli.cloud">{t("Open Console")} <span aria-hidden="true">↗</span></a></div><span className="planned">{t("By invitation")}</span></section>
      <section className="closing wrap"><p>{t("Got something in mind?")}</p><h2 className="whitespace-pre-line">{page.closingTitle}</h2><a className="button" href={contact}>{page.closingButton} <span aria-hidden="true">↗</span></a><span className="closing-mark" aria-hidden="true"><Mark/></span></section>
    </main>
    <footer className="wrap site-footer"><a className="brand" href="#" aria-label={t("Spitzli Cloud home")}><Mark/>{"Spitzli Cloud"}</a><p>{t("A project by")} <a href="https://spitzli.dev" target="_blank" rel="noopener noreferrer">{"Spitzli Development"}</a></p><a href={`mailto:${page.contactEmail}`}>{page.contactEmail} <span aria-hidden="true">↗</span></a><nav aria-label={t("Legal")}><IntentLink href="/terms">{t("Terms")}</IntentLink><IntentLink href="/privacy">{t("Privacy")}</IntentLink><IntentLink href="/imprint">{t("Legal notice")}</IntentLink></nav><ThemePicker/><LanguagePicker className="language-picker"/></footer>
  </>;
}
