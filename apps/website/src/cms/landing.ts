import {msgid} from '@webdock/i18n';
import type { Field, GlobalConfig } from 'payload';
import { faqs } from '../lib/content';
import { authenticated } from './access';

export const landingDefaults = {
  seoTitle: msgid("Spitzli Cloud — Websites, applications and cloud services"),
  seoDescription: msgid("Your projects in one place. Websites and applications by Spitzli — on spitzli.site or your own domain."),
  contactEmail: 'dominik@spitzli.dev',
  heroTitle: msgid("Your projects.\nOne place."),
  heroDescription: msgid("Websites, applications and the services behind them — connected in Spitzli Cloud."),
  heroButton: msgid("Discuss your project"),
  heroNote: msgid("Your address on spitzli.site. Or your own domain."),
  conceptTitle: msgid("From your idea\nto your application."),
  conceptDescription: msgid("Bring your projects, their services and their public addresses together in one workspace."),
  useCases: [
    { title: msgid("Keep your projects connected."), description: msgid("Manage your websites, applications and the services they use from one place.") },
    { title: msgid("Your website. Your place."), description: msgid("No domain of your own? Your public project gets an address on spitzli.site. Easy to share, easy to find again.") },
    { title: msgid("Ready for what comes next."), description: msgid("Connect a domain you own when your project is ready. Your application and its data stay in place.") },
  ],
  aboutTitle: msgid("Your projects.\nA direct connection."),
  aboutDescription: msgid("Spitzli Cloud is run by Dominik at Spitzli Development. From your application to its infrastructure, you work directly with the person building it."),
  faqTitle: msgid("Good questions.\nStraight answers."),
  faqs,
  outlookTitle: msgid("Your projects. One workspace."),
  outlookDescription: msgid("Spitzli Cloud Console brings your projects, applications and services together. Access is by invitation."),
  closingTitle: msgid("There’s a place\nfor your next idea."),
  closingButton: msgid("Let’s talk about it"),
};

const text = (name: Exclude<keyof typeof landingDefaults, 'faqs' | 'useCases'>, label: string, multiline = false): Field => {
  const base = { name, label, required: true, defaultValue: landingDefaults[name] };
  return multiline ? { ...base, type: 'textarea' } : { ...base, type: 'text' };
};

export const LandingPage: GlobalConfig = {
  slug: 'landing-page', label: msgid("Landing page"),
  access: { read: authenticated, update: authenticated },
  fields: [
    { type: 'tabs', tabs: [
      { label: msgid("SEO & contact"), fields: [text('seoTitle', msgid("Page title")), text('seoDescription', msgid("Description"), true), { name: 'contactEmail', type: 'email', required: true, defaultValue: landingDefaults.contactEmail }] },
      { label: msgid("Hero"), fields: [text('heroTitle', msgid("Heading (one line per row)"), true), text('heroDescription', msgid("Description"), true), text('heroButton', msgid("Button label")), text('heroNote', msgid("Supporting note"))] },
      { label: msgid("Concept"), fields: [text('conceptTitle', msgid("Heading"), true), text('conceptDescription', msgid("Description"), true), { name: 'useCases', type: 'array', required: true, minRows: 1, maxRows: 6, defaultValue: landingDefaults.useCases, fields: [{ name: 'title', type: 'text', required: true }, { name: 'description', type: 'textarea', required: true }] }] },
      { label: msgid("About"), fields: [text('aboutTitle', msgid("Heading"), true), text('aboutDescription', msgid("Description"), true)] },
      { label: msgid("FAQ"), fields: [text('faqTitle', msgid("Heading"), true), { name: 'faqs', type: 'array', required: true, minRows: 1, maxRows: 20, defaultValue: faqs, fields: [{ name: 'question', type: 'text', required: true }, { name: 'answer', type: 'textarea', required: true }] }] },
      { label: msgid("Outlook & closing"), fields: [text('outlookTitle', msgid("Outlook heading")), text('outlookDescription', msgid("Outlook description"), true), text('closingTitle', msgid("Closing heading"), true), text('closingButton', msgid("Button label"))] },
    ] },
  ],
};
