import { IntentLink } from '@/components/intent-link';
import { getRequestI18n } from '@webdock/i18n/next';
import type { Metadata } from 'next';

export async function generateMetadata(): Promise<Metadata> {
  const { t } = await getRequestI18n();
  return {
    title: t("Terms and conditions — Spitzli Cloud"),
    description: t("Terms for Spitzli Cloud services, including consumer cancellation and withdrawal rights."),
    alternates: { canonical: '/terms' },
    openGraph: { title: t("Terms and conditions — Spitzli Cloud"), url: '/terms' },
  };
}

export default async function Terms() {
  const { t, date } = await getRequestI18n();
  return <article>
    <p className="subtle-label">{t("Spitzli Cloud · Updated {date}", { date: date("2026-10-14T12:00:00Z", { dateStyle: "long" }) })}</p>
    <h1>{t("Terms and conditions")}</h1>
    <p>{t("These terms apply to Spitzli Cloud services for consumers and businesses, including agreed website development, project management, hosting, databases and Mail. A consumer acts mainly outside their trade or profession. Individually agreed terms take precedence; mandatory consumer rights remain unaffected.")}</p>
    <h2>{t("1. Provider and contract")}</h2>
    <p>{t("Dominik Spitzli, trading as Spitzli Development, Emsoldstrasse 40, 26180 Rastede, Germany.")}</p>
    <p><a href="mailto:dominik@spitzli.dev">dominik@spitzli.dev</a> · <a href="tel:+4915679657952">+49 1567 9657952</a> · <IntentLink href="/imprint">{t("Full provider details")}</IntentLink></p>
    <p>{t("The service description and accepted offer determine what is supplied, the price, payment dates, term and any service-level agreement. A contract is formed when you accept our binding offer or we confirm your order. Website descriptions alone are not binding offers. Creating an account or connecting an integration does not itself create a paid subscription. We provide the agreed terms and required consumer information in a durable form, for example by email.")}</p>
    <h2>{t("2. Services and availability")}</h2>
    <p>{t("Only the functions and resources included in the accepted offer are owed. Preview or beta features are identified as such and do not expand the agreed service. We maintain and secure the services; maintenance, incidents and external providers can affect availability. A specific uptime, response time, backup schedule or recovery period applies only if agreed. This does not limit statutory rights concerning defects, security updates or non-performance.")}</p>
    <p>{t("Optional services must be enabled separately. Incoming Mail is provisioned only when explicitly activated; outgoing-only Mail does not include a mailbox. Successful handoff to a mail relay does not guarantee delivery to the recipient’s inbox. When you connect your own provider account, its contract and charges continue to apply. We remain responsible for the obligations we have agreed to perform.")}</p>
    <h2>{t("3. Prices and payment")}</h2>
    <p>{t("The accepted offer states the price, currency, billing period and any usage charges. Prices offered to consumers include applicable VAT and other mandatory price components. Additional paid work or resources require your agreement. Invoices are due on the agreed date; statutory default rules apply. Changes to agreed prices require a contractual basis and cannot be imposed merely by changing this page.")}</p>
    <h2>{t("4. Your responsibilities and content")}</h2>
    <p>{t("Provide accurate account and billing details, protect access credentials and grant only authorised people access. Connect only systems you are entitled to manage. Report suspected misuse promptly. Do not distribute unlawful content, malware or unsolicited advertising, infringe third-party rights, bypass access controls or deliberately disrupt services. For email, you are responsible for lawful recipient lists and any consent needed for sending or open and click tracking.")}</p>
    <p>{t("You retain your rights to your content. You grant us only the rights needed to host, process and deliver it for the agreed service. Rights to individually developed work are governed by the project agreement; third-party and open-source licences remain applicable. Keep independent copies of important data and arrange any required backups with us. Your own precautions do not release us from agreed backup obligations or statutory liability.")}</p>
    <h2>{t("5. Privacy and data processing")}</h2>
    <p>{t("Our handling of personal data is described in the privacy notice. Where we process personal data on your behalf, a separate data-processing agreement is required before that processing begins. You remain responsible for the lawfulness of your content, instructions and information to affected individuals.")} <IntentLink href="/privacy">{t("Read the privacy notice")}</IntentLink>.</p>
    <h2>{t("6. Term, cancellation and suspension")}</h2>
    <p>{t("The offer specifies the term and notice period. If no term is agreed for an ongoing service, it runs indefinitely and can be cancelled at any time with one month’s notice. For consumers, any tacit renewal after an initial term is indefinite and cancellable with no more than one month’s notice. Statutory rights to cancel earlier or for good cause remain unaffected. You can give notice by email to the contact above; any legally required online cancellation facility remains an additional option.")}</p>
    <p>{t("We may restrict access where proportionate and necessary because of unlawful use, a concrete security risk or a substantial contractual breach. Where feasible, we first explain the reason and allow a reasonable opportunity to remedy it; urgent protective measures may be immediate. We restore access when the reason ends. Payment-related suspension requires the statutory prerequisites.")}</p>
    <p>{t("Arrange an export of needed data before the service ends. We coordinate return or deletion under the service and data-processing agreements and applicable law. Deactivation or suspension is not deletion; retained data remains protected. Removing a Spitzli Cloud connection does not automatically cancel or revoke your account with a third-party provider.")}</p>
    <h2>{t("7. Defects and liability")}</h2>
    <p>{t("Statutory rights concerning defects and statutory liability apply. Please report faults with enough information for us to investigate; this does not restrict your rights. Nothing in these terms excludes liability for intent, gross negligence, injury to life, body or health, or other mandatory liability. No general exclusion of liability or blanket transfer of third-party claims to you is agreed.")}</p>
    <h2 id="withdrawal">{t("8. Consumer right of withdrawal")}</h2>
    <p>{t("For service contracts concluded at a distance or away from business premises, consumers have the following right of withdrawal. Business customers do not have this statutory consumer right.")}</p>
    <p>{t("You may withdraw from the contract without giving a reason within 14 days from the day it is concluded. To withdraw, send a clear statement of your decision, for example by email or post, to Dominik Spitzli, Spitzli Development, Emsoldstrasse 40, 26180 Rastede, Germany, dominik@spitzli.dev. You may use the model form below, but do not have to. Sending your statement before the deadline is sufficient.")}</p>
    <p>{t("Phone")}: <a href="tel:+4915679657952">+49 1567 9657952</a></p>
    <p>{t("If you withdraw, we refund all payments received from you for the contract without undue delay and no later than 14 days after receiving your withdrawal. We use the original payment method unless expressly agreed otherwise; we charge no refund fees. If you expressly requested that services begin during the withdrawal period and were properly informed, you owe a proportionate amount for the services provided up to your withdrawal compared with the full agreed service.")}</p>
    <p>{t("For a paid service, the right expires early only after full performance and only if you expressly agreed to an early start and acknowledged beforehand that full performance would end your right of withdrawal. Starting an ongoing hosting service alone does not end that right.")}</p>
    <h2>{t("Model withdrawal form")}</h2>
    <p>{t("Complete and send this form only if you wish to withdraw from the contract.")}</p>
    <p>{t("To: Dominik Spitzli, Spitzli Development, Emsoldstrasse 40, 26180 Rastede, Germany; dominik@spitzli.dev.")}</p>
    <p className="whitespace-pre-line">{t("I/We (*) hereby withdraw from the contract concluded by me/us (*) for the provision of the following service: ____________________\nOrdered on: ____________________\nName of consumer(s): ____________________\nAddress of consumer(s): ____________________\nSignature of consumer(s), only if sent on paper: ____________________\nDate: ____________________\n(*) Delete as appropriate.")}</p>
    <h2>{t("9. Applicable law and changes")}</h2>
    <p>{t("German law applies. For consumers, this choice does not remove mandatory protections of the country in which they habitually reside. Statutory jurisdiction rules apply. Changes to these terms do not automatically amend an existing contract; amendments require a valid agreement or legal basis. We inform you about relevant changes in advance.")}</p>
  </article>;
}
