"use client";
import type { ComponentProps } from "react";
import {
  AddDatabase as Add,
  DatabaseAccess as Access,
} from "@webdock/module-databases/ui/forms";
import { DatabaseActionBoundary } from "./action-boundary";
export function AddDatabase(props: ComponentProps<typeof Add>) {
  return (
    <DatabaseActionBoundary key={props.projects.map((p) => p.id).join(":")}>
      <Add {...props} />
    </DatabaseActionBoundary>
  );
}
export function DatabaseAccess(props: ComponentProps<typeof Access>) {
  return (
    <DatabaseActionBoundary key={props.bindingID}>
      <Access {...props} />
    </DatabaseActionBoundary>
  );
}
