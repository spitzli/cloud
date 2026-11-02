import { mailRoute } from "../../../../../../../modules/mail/product/api";
export const GET = mailRoute("webhooks.get");
export const PATCH = mailRoute("webhooks.update");
export const DELETE = mailRoute("webhooks.delete");
