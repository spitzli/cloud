import { mailRoute } from "../../../../../../modules/mail/product/api";
export const GET = mailRoute("emails.list");
export const POST = mailRoute("emails.send");
