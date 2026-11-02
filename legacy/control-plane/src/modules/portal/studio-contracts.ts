// Public JSON data returned by Auth's fixed Studio API. Keep this file dependency-free.
// Dates crossing this boundary are ISO strings; native sessions and credentials are excluded.
export type ChangeResult = { message: string };
export type NativeMailView = {
  service: import("@webdock/mail-core").MailService;
  mailboxesAvailable?: boolean;
  sendingAvailable?: boolean;
  canActivate: boolean;
  canSuspend: boolean;
  canReconcile: boolean;
} | null;
export type Site = {
  id: string;
  name: string;
  url: string;
  role: string;
  customerID?: string;
  customerName?: string;
};
export type AccountSummary = {
  id: string;
  name: string;
  email: string;
  role: string;
  banned: boolean | null;
  emailVerified: boolean;
  mustChangePassword: boolean;
};
export type TenantSummary = {
  id: string;
  name: string;
  status: string;
  role: string | null;
  capabilities?: {
    manageProjects: boolean;
    createDeployments: boolean;
    manageDatabases: boolean;
    manageBilling: boolean;
  };
};
export type TenantInvitation = { id: string; role: string; name: string };
export type TenantProfileField =
  | "first_name"
  | "last_name"
  | "company_name"
  | "contact_name"
  | "contact_email"
  | "phone"
  | "address_line1"
  | "address_line2"
  | "postal_code"
  | "city"
  | "region"
  | "country";
export type TenantProfile = {
  id: string;
  name: string;
  status: string;
  customer_type: string;
  organization_id: string;
  member_role: string | null;
} & Record<TenantProfileField, string | null>;
export type TenantView = {
  team?: {provider:"clerk";ready:boolean;canManage:boolean};
  tenant: TenantProfile;
  canManage: boolean;
  operator: boolean;
  sites: Site[];
  members: {
    id: string;
    role: string;
    name: string;
    email: string;
    banned: boolean | null;
    emailVerified: boolean;
    mustChangePassword: boolean;
  }[];
  invitations: {
    id: string;
    email: string;
    role: string;
    status: string;
    expiresAt: string;
  }[];
};
export type AccessView = {
  users: AccountSummary[];
  organizations: { id: string; name: string }[];
  websites: {
    id: string;
    label: string;
    organization_id: string | null;
    redirectUris: string[];
  }[];
  grants: {
    id: string;
    role: string;
    enabled: boolean;
    name: string;
    email: string;
    label: string;
  }[];
  invitations: {
    id: string;
    email: string;
    status: string;
    expiresAt: string;
    name: string;
  }[];
  events: {
    id: string;
    actor_id: string;
    action: string;
    target_id: string | null;
    outcome: string;
    created_at: string;
    actor: string | null;
  }[];
};
export type Allowances = Record<
  "storageBytes" | "mailMessages" | "transferBytes" | "websites" | "editors",
  number | null
> & { hosting?: import("@webdock/hosting-contracts").HostingAllowances };
export type Plan = {
  id: string;
  name: string;
  description: string;
  allowances: Allowances;
  createdAt: string;
};
export type TenantPlanView = {
  subscription: {
    name: string;
    description: string;
    base: Allowances;
    extras: Allowances;
    effective: Allowances;
    revision: number;
  } | null;
  revision: number;
  offers: {
    id: string;
    name: string;
    status: string;
    expiresAt: string;
    acceptedAt?: string;
  }[];
};
export type OfferView = {
  customerID: string;
  customerName: string;
  name: string;
  description: string;
  terms: string;
  base: Allowances;
  extras: Allowances;
  effective: Allowances;
  status: string;
  expiresAt: string;
  canAccept: boolean;
};
export type PlatformSettings = {
  name: string;
  supportEmail: string;
  cmsEnabled: boolean;
  mailEnabled: boolean;
  mailGatewayEnabled: boolean;
  mailDispatchEnabled: boolean;
  mailWebhooksEnabled: boolean;
  mailSendingIP: string;
  mailDefaultLimit: number;
  mailRegion: "eu" | "global";
};
export type MailView = {
  tenant: { id: string; name: string; status: string };
  operator: boolean;
  canManage: boolean;
  enabled: boolean;
  configured: boolean;
  region: "eu" | "global";
  defaultLimit: number;
  account: null | {
    state: string;
    providerID: string | null;
    email: string;
    active?: boolean;
    limit?: number;
    sent?: number;
    interval?: string;
    checkedAt?: string;
    issue?: string;
  };
  domains: {
    id: string;
    domain: string;
    status: "pending" | "ownership_verified";
    token: string;
    checkedAt?: string;
  }[];
};
export type TenantSenderDomain = {
  id: string;
  domain: string;
  status: "pending" | "ownership_verified";
  token: string;
  checkedAt?: string;
  providerStatus: "not_registered" | "pending" | "verified";
  providerID?: string;
  providerCheckedAt?: string;
  spfVerified?: boolean;
  dkimVerified?: boolean;
  dmarcVerified?: boolean;
  records: DnsRecord[];
  dnsIssue?: string;
  busy: boolean;
};

export type MailKeyInput = {
  action: string;
  label?: string;
  permissions?: string[];
  ips?: string[];
  consumerKey?: string;
  confirm?: string;
};

export type DnsRecord = {
  record: "OWNERSHIP" | "SPF" | "DKIM" | "DMARC";
  type: "TXT";
  name: string;
  host: string;
  value: string;
  current: string[];
  status: "verified" | "missing" | "conflict";
  action: "none" | "add" | "update";
};

export type TrackingTool = {
  id: string;
  forced: boolean | null;
  enabled: boolean;
  settings: { key: string; value: boolean | string | number }[];
};
export type TrackingDomain = {
  id: string;
  domain_name: string;
  verification_domain: string;
  verified: boolean;
  ssl: boolean;
  enabled: boolean;
  default: boolean;
};
export type TrackingInfo = {
  all_domains_disabled: boolean;
  match_sender: boolean;
  no_default_domain: boolean;
};
export type TrackingSettings = {
  click: TrackingTool;
  opening: TrackingTool;
  info: TrackingInfo;
  custom: boolean | null;
};
export type TrackingView = {
  tenant: { name: string };
  canManage: boolean;
  unavailable: string | null;
  senders: { id: string; domain: string }[];
  settings?: TrackingSettings;
  checkedAt?: string;
  domains: {
    id: string;
    domain: string;
    mapped: boolean;
    cancellable: boolean;
    snapshot: Partial<TrackingDomain>;
  }[];
};
export type MailKeysView = {
  tenant: { id: string; name: string };
  operator: boolean;
  enabled: boolean;
  active: boolean;
  smtpHost: string;
  sendAPI: string;
  keys: {
    consumerKey: string;
    label: string;
    creation_time: string;
    ips: string[];
    is_legacy: boolean;
    permissions: string[];
  }[];
};
export type StorageView = {
  stores: {
    id: string;
    label: string;
    environment: "production" | "preview";
    prefix: string;
    storeID: string;
    bytes: number | null;
    objects: number | null;
    checkedAt: string | null;
    error: string | null;
  }[];
  productionBytes: number | null;
  previewBytes: number | null;
};

export type NativeMailboxesView = {
  customerID: string;
  operator: boolean;
  canManage: boolean;
  canConfigure: boolean;
  configuration: {
    revision: string;
    appliedRevision: string;
    state: string;
  } | null;
  mailboxes: {
    id: string;
    name: string;
    address: string;
    state: string;
    enabled: boolean;
    quotaBytes: string;
    canOpen: boolean;
  }[];
  domains: {
    id: string;
    name: string;
    state: string;
    dkimKeys: { id: string; selector: string; publicKey: string }[];
  }[];
  verifiedDomains: { id: string; domain: string }[];
  people: { id: string; name: string; email: string }[];
  grants: {
    mailboxID: string;
    subject: string;
    enabled: boolean;
    revision: string;
  }[];
};
