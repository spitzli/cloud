
import { getRequestI18n } from '@webdock/i18n/next';
import { customerFields } from "../../../../../lib/customer-fields";
import Link from "next/link";
import { Editor } from "../../../../../components/editor";
import { saveCustomer } from "../../../../../lib/actions";
export default async function NewCustomer() {
  const i18n = await getRequestI18n();

  return (
    <>
      <Link className="back" href="/customers">{i18n.t("← Customers")}</Link>
      <div className="page-heading">
        <div>
          <h1>{i18n.t("New customer")}</h1>
          <p>{i18n.t("Each customer gets a tenant for their projects. Access is invite-only; add members after creating the customer.")}</p>
        </div>
      </div>
      <section className="panel">
        <Editor
          action={saveCustomer.bind(null, null)}
          submit={i18n.t("Create customer")}
          fields={customerFields()}
        />
      </section>
    </>
  );
}
