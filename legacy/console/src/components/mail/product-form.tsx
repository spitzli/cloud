"use client";
import {
  MailProductForm as Form,
  type MailProductFormProps,
} from "@webdock/module-mail/ui/product-form";
import { mailProductAction } from "./product-actions";
export { CopyMailValue } from "@webdock/module-mail/ui/product-form";
export type { MailField } from "@webdock/module-mail/ui/product-form";
export function MailProductForm(props: Omit<MailProductFormProps, "action">) {
  return <Form key={`${props.customer}:${props.connection ?? ""}:${props.operation}`} {...props} action={mailProductAction} />;
}
