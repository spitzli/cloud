import type { EventType } from "@spitzli/mail-contracts";
export type RecipientState = {
  delivery:
    "pending" | "deferred" | "delivered" | "bounced" | "failed" | "suppressed";
  firstDeliveredAt: string | null;
  firstOpenedAt: string | null;
  lastOpenedAt: string | null;
  firstClickedAt: string | null;
  lastClickedAt: string | null;
  complainedAt: string | null;
  unsubscribedAt: string | null;
};
export function emptyRecipientState(): RecipientState {
  return {
    delivery: "pending",
    firstDeliveredAt: null,
    firstOpenedAt: null,
    lastOpenedAt: null,
    firstClickedAt: null,
    lastClickedAt: null,
    complainedAt: null,
    unsubscribedAt: null,
  };
}
export function projectRecipient(
  previous: RecipientState,
  event: { type: EventType; occurredAt: string },
): RecipientState {
  const next = { ...previous },
    time = new Date(event.occurredAt).toISOString();
  const first = (old: string | null) => (old && old < time ? old : time);
  const last = (old: string | null) => (old && old > time ? old : time);
  switch (event.type) {
    case "email.delivery_delayed":
      if (["pending", "deferred"].includes(next.delivery))
        next.delivery = "deferred";
      break;
    case "email.delivered":
      if (!["bounced", "suppressed", "failed"].includes(next.delivery))
        next.delivery = "delivered";
      next.firstDeliveredAt = first(next.firstDeliveredAt);
      break;
    case "email.bounced":
      if (next.delivery !== "suppressed") next.delivery = "bounced";
      break;
    case "email.failed":
      if (["pending", "deferred"].includes(next.delivery))
        next.delivery = "failed";
      break;
    case "email.suppressed":
      if (next.delivery === "pending") next.delivery = "suppressed";
      break;
    case "email.opened":
      next.firstOpenedAt = first(next.firstOpenedAt);
      next.lastOpenedAt = last(next.lastOpenedAt);
      break;
    case "email.clicked":
      next.firstClickedAt = first(next.firstClickedAt);
      next.lastClickedAt = last(next.lastClickedAt);
      break;
    case "email.complained":
      next.complainedAt = first(next.complainedAt);
      break;
    case "email.unsubscribed":
      next.unsubscribedAt = first(next.unsubscribedAt);
      break;
  }
  return next;
}
