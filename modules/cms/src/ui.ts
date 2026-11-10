import { msgid } from "@spitzli/i18n";
// Public presentation metadata only. Bridge credentials must stay on the server.
export type CmsKind = "demo" | "shop";
export type CmsComponentID =
  | "content-editor"
  | "calendar-editor"
  | "product-variants-editor"
  | "promotion-editor"
  | "media-picker"
  | "featured-products-editor"
  | "customer-profile-editor"
  | "canvas-editor";
export const cmsComponents: Record<
  CmsComponentID,
  { label: string; description: string }
> = {
  "canvas-editor": {
    label: msgid("Free canvas"),
    description: msgid("Design pages freely and preview drafts live."),
  },
  "content-editor": {
    label: msgid("Content editor"),
    description: msgid("Edit website text and images."),
  },
  "calendar-editor": {
    label: msgid("Calendar editor"),
    description: msgid("Manage opening hours, exceptions and appointments."),
  },
  "product-variants-editor": {
    label: msgid("Product and variant editor"),
    description: msgid("Edit products, variants, extras and prices."),
  },
  "media-picker": {
    label: msgid("Media picker"),
    description: msgid("Select existing images for products and the homepage."),
  },
  "featured-products-editor": {
    label: msgid("Featured products"),
    description: msgid("Select the featured products on the homepage."),
  },
  "customer-profile-editor": {
    label: msgid("Customer profiles"),
    description: msgid("Edit customer names and saved addresses."),
  },
  "promotion-editor": {
    label: msgid("Daily promotion editor"),
    description: msgid("Manage the demo daily promotion and discount."),
  },
};
type ModuleDefinition = {
  id: string;
  label: string;
  description: string;
  path: string;
  kind: CmsKind;
  components: CmsComponentID[];
  managementOnly?: boolean;
  readOnly?: boolean;
};
const modules: ModuleDefinition[] = [
  {
    id: "content",
    label: msgid("Content"),
    description: msgid(
      "Edit text and images. Saved changes appear immediately on the website.",
    ),
    path: "/content",
    kind: "demo",
    components: ["content-editor"],
  },
  {
    id: "requests",
    label: msgid("Requests"),
    description: msgid(
      "View incoming demo requests and edit their status and internal notes.",
    ),
    path: "/requests",
    kind: "demo",
    components: [],
  },
  {
    id: "calendar",
    label: msgid("Appointments"),
    description: msgid("Manage opening hours, availability and appointments."),
    path: "/calendar",
    kind: "demo",
    components: ["calendar-editor"],
  },
  {
    id: "products",
    label: msgid("Products"),
    description: msgid(
      "Manage existing products, variants, extras, prices and availability.",
    ),
    path: "/shop?kind=products",
    kind: "shop",
    components: ["product-variants-editor", "media-picker"],
  },
  {
    id: "settings",
    label: msgid("Website & content"),
    description: msgid("Edit the homepage and business information."),
    path: "/shop?kind=settings",
    kind: "shop",
    components: [
      "content-editor",
      "promotion-editor",
      "media-picker",
      "featured-products-editor",
    ],
  },
  {
    id: "media",
    label: msgid("Media"),
    description: msgid("Browse existing images for image selection."),
    path: "/shop?kind=media",
    kind: "shop",
    components: ["media-picker"],
    readOnly: true,
  },
  {
    id: "orders",
    label: msgid("Orders"),
    description: msgid(
      "View simulated orders and their items. This area is read-only.",
    ),
    path: "/shop?kind=orders",
    kind: "shop",
    components: [],
    managementOnly: true,
    readOnly: true,
  },
  {
    id: "customers",
    label: msgid("Shop customers"),
    description: msgid(
      "Manage customer names and addresses. Sign-in and permissions remain protected.",
    ),
    path: "/shop?kind=customers",
    kind: "shop",
    components: ["customer-profile-editor"],
    managementOnly: true,
  },
];
export function cmsRoleLabel(role: string): string {
  return (
    (
      {
        operator: msgid("Superadmin"),
        admin: msgid("Tenant admin"),
        editor: msgid("Editor"),
        reader: msgid("Read-only"),
      } as Record<string, string>
    )[role] || msgid("No access")
  );
}
export function cmsModules(
  kind: CmsKind,
  role: string,
  canvas = false,
  promotions = true,
  shop = false,
) {
  if (!["operator", "admin", "editor", "reader"].includes(role)) return [];
  const manager = role === "operator" || role === "admin";
  const available: ModuleDefinition[] = canvas
    ? [
        {
          id: "builder",
          label: msgid("Design"),
          description: msgid(
            "Design free-form pages, save drafts and publish.",
          ),
          path: "/builder",
          kind,
          components: ["canvas-editor"],
        },
        ...modules,
      ]
    : modules;
  return available
    .filter(
      (module) =>
        (module.kind === kind || (shop && module.kind === "shop")) &&
        (!module.managementOnly || manager),
    )
    .map((module) => ({
      ...module,
      label:
        kind === "demo" && shop && module.id === "settings"
          ? msgid("Shop settings")
          : module.label,
      description:
        kind === "demo" && shop && module.id === "settings"
          ? msgid("Edit contact details for the demo checkout.")
          : module.description,
      components:
        kind === "demo" && shop && module.id === "settings"
          ? ["content-editor" as CmsComponentID]
          : module.components.filter(
              (id) => promotions || id !== "promotion-editor",
            ),
      writable: role !== "reader" && !module.readOnly,
    }));
}
export function cmsHubURL(bindingID: string) {
  return `/sites/${encodeURIComponent(bindingID)}`;
}

export function cmsSiteName(name: string) {
  return (
    name.replace(/\s*(?:·\s*)?(?:Demo-CMS|Website-Demo|CMS)$/i, "").trim() ||
    name
  );
}
