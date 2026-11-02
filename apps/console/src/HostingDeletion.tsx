import { useState } from "react";
import { useMutation } from "@tanstack/react-query";
import { useBlocker } from "@tanstack/react-router";
import { formatResource } from "@webdock/hosting-contracts";
import { Modal } from "./common";
import { deleteCommand, deletionPlanSchema, type DeletionPlan, type HostingApp } from "./hosting-contracts";
import { useHostingAction, useHostingCommand } from "./hosting-client";
import { HostingReceipt } from "./HostingOperation";
import { ActionFailure } from "./HostingActionState";
import * as ui from "./ui";
export function HostingDeletion({ app }: { app: HostingApp }) {
  const call = useHostingCommand();
  const { mutation, retry, uncertain } = useHostingAction();
  const [review, setReview] = useState<DeletionPlan>();
  const [typed, setTyped] = useState("");
  // The server derives the plan; its hash binds the confirmation to exactly what was shown.
  const plan = useMutation({
    mutationFn: () => call({ action: "apps.deletion", appID: app.id }, deletionPlanSchema),
    onSuccess: (value) => { setTyped(""); setReview(value); },
  });
  useBlocker({
    shouldBlockFn: () => uncertain && !window.confirm("The deletion outcome is unknown. Leave this view and review the server state?"),
    enableBeforeUnload: uncertain,
  });
  const locked = plan.isPending || mutation.isPending || uncertain || Boolean(mutation.data) || app.status === "pending";
  return <section className={ui.cx(`${ui.card} mt-6 border-danger/40`)}>
    <div className={ui.cardBody}>
      <h2 className={ui.sectionTitle}>Delete application</h2>
      <p className="-mt-2 text-sm text-muted">Deletion removes the runtime resources after review. Retained storage is listed separately in the plan.</p>
      {plan.error ? <p className="text-sm text-danger" role="alert">{plan.error.message}</p> : null}
      <ActionFailure error={mutation.error} uncertain={uncertain} retry={retry} pending={mutation.isPending} />
      {mutation.data ? <HostingReceipt receipt={mutation.data} /> : null}
    </div>
    <div className={ui.cx(`${ui.cardFooter} justify-end border-danger/40 bg-danger-soft`)}>
      <button className={ui.cx(`${ui.button()} border-danger/50 text-danger`)} disabled={locked} onClick={() => { mutation.reset(); plan.mutate(); }}>
        {plan.isPending ? "Loading deletion plan…" : "Review deletion"}
      </button>
    </div>
    {review ? <Modal title="Delete application" close={() => { setReview(undefined); }}>
      <form className="grid gap-5 text-sm" onSubmit={(event) => {
        event.preventDefault();
        mutation.mutate(deleteCommand(app, review, typed, crypto.randomUUID()));
        setReview(undefined);
      }}>
        <p>This removes {review.plan.resources.length ? review.plan.resources.join(", ") : "the unpublished draft"} for <strong>{review.plan.name}</strong>.</p>
        {review.plan.persistentData ? <p className="text-danger">Persistent storage ({formatResource("volumeBytes", review.plan.volumeBytes)}) is retained after deletion and must be removed separately.</p> : null}
        <label className={ui.field}>Type the application name to confirm<input className={ui.input} value={typed} autoComplete="off" spellCheck={false} onChange={(event) => { setTyped(event.target.value); }} /></label>
        <button className={ui.cx(`${ui.button("danger")} justify-self-start`)} disabled={typed !== app.name || review.plan.name !== app.name}>Confirm deletion</button>
      </form>
    </Modal> : null}
  </section>;
}
