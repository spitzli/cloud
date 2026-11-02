
import { msgid } from '@webdock/i18n';
import type { EditorField } from "../components/editor";
import type { Customer } from "../payload-types";

export function customerFields(customer?: Customer): EditorField[] {
  const fields: EditorField[] = [
    { name: "name", label: msgid("Customer display name"), required: true, group: msgid("Customer profile") },
    { name: "customerType", label: msgid("Customer type"), type: "select", value: "company", options: [{ value: "company", label: msgid("Company") }, { value: "person", label: msgid("Person") }] },
    { name: "companyName", label: msgid("Company name"), hint: msgid("For business customers.") },
    { name: "firstName", label: msgid("First name"), hint: msgid("For personal customers or the primary contact.") },
    { name: "lastName", label: msgid("Last name") },
    { name: "contactName", label: msgid("Contact person"), group: msgid("Contact details") },
    { name: "contactEmail", label: msgid("Contact email"), type: "email", maxLength: 254, hint: msgid("A contact address does not grant access. Invite members from Tenant & invitations after saving.") },
    { name: "phone", label: msgid("Phone"), type: "tel", maxLength: 50 },
    { name: "addressLine1", label: msgid("Street and number"), group: msgid("Postal address") },
    { name: "addressLine2", label: msgid("Address addition") },
    { name: "postalCode", label: msgid("Postal code"), maxLength: 32 },
    { name: "city", label: msgid("City") },
    { name: "region", label: msgid("State / region") },
    { name: "country", label: msgid("Country code"), maxLength: 2, hint: msgid("Two-letter ISO code, for example DE or CH.") },
    { name: "notes", label: msgid("Private operator notes"), type: "textarea", group: msgid("Internal"), hint: msgid("Only operators can see these notes.") },
  ];
  return fields.map(field => ({ ...field, value: customer?.[field.name as keyof Customer] ?? field.value ?? "" }));
}
