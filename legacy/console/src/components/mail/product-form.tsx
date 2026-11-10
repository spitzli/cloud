"use client";
import {
  MailProductForm as Form,
  type MailProductFormProps,
} from "@spitzli/module-mail/ui/product-form";
import { mailProductAction } from "./product-actions";
export { CopyMailValue } from "@spitzli/module-mail/ui/product-form";
export type { MailField } from "@spitzli/module-mail/ui/product-form";
export function MailProductForm(props: Omit<MailProductFormProps, "action">) {
  return <Form key={`${props.customer}:${props.connection ?? ""}:${props.operation}`} {...props} action={mailProductAction} />;
}
