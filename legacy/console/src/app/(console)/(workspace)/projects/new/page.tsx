
import { msgid } from '@webdock/i18n';

import { getRequestI18n } from '@webdock/i18n/next';
import Link from "next/link";
import { Editor } from "../../../../../components/editor";
import { saveProject } from "../../../../../lib/actions";
import { requireRegistryOperator } from "../../../../../lib/registry-page";
export default async function NewProject({
  searchParams,
}: {
  searchParams: Promise<{ customer?: string }>;
}) {
  const i18n = await getRequestI18n();

  const { payload, user } = await requireRegistryOperator();
  const q = await searchParams;
  const customers = await payload.find({
    collection: "customers",
    where: { status: { equals: "active" } },
    limit: 1000,
    sort: "name",
    overrideAccess: false,
    user,
  });
  return (
    <>
      <Link className="back" href="/">{i18n.t("← Projects")}</Link>
      <div className="page-heading">
        <div>
          <h1>{i18n.t("New project")}</h1>
          <p>{i18n.t("Start with the essentials. No CMS will be created automatically.")}</p>
        </div>
      </div>
      <section className="panel">
        {customers.totalDocs ? (
          <Editor
            action={saveProject.bind(null, null)}
            submit={i18n.t("Create project")}
            fields={[
              { name: "name", label: msgid("Project name"), required: true },
              {
                name: "customer",
                label: msgid("Customer"),
                type: "select",
                required: true,
                value: q.customer,
                options: [
                  { value: "", label: msgid("Choose a customer") },
                  ...customers.docs.map((c) => ({
                    value: c.id,
                    label: c.name, translate: false,
                  })),
                ],
              },
              { name: "url", label: msgid("Website URL"), type: "url" },
              { name: "repositoryURL", label: msgid("Repository URL"), type: "url" },
              { name: "notes", label: msgid("Notes"), type: "textarea" },
            ]}
          />
        ) : (
          <div className="empty">
            <h2>{i18n.t("Add a customer first.")}</h2>
            <Link className="button" href="/customers/new">{i18n.t("Create customer")}</Link>
          </div>
        )}
      </section>
    </>
  );
}
