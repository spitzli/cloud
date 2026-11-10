import { HostingError } from "@spitzli/hosting-contracts";
export function assertSetupConfirmation(input: {
  setupRevision: number;
  submittedRevision: number;
  previewID: string;
  submittedPreviewID: string;
  expiresAt: Date;
  now: Date;
  requiredOfferIDs: string[];
  acceptedOfferIDs: string[];
}) {
  if (
    input.setupRevision !== input.submittedRevision ||
    input.previewID !== input.submittedPreviewID
  )
    throw new HostingError(
      409,
      "Setup changed. Review the current configuration.",
    );
  if (
    !Number.isFinite(input.expiresAt.getTime()) ||
    input.now >= input.expiresAt
  )
    throw new HostingError(409, "Setup preview expired. Review it again.");
  if (
    input.requiredOfferIDs.length !== input.acceptedOfferIDs.length ||
    !input.requiredOfferIDs.every((id) => input.acceptedOfferIDs.includes(id))
  )
    throw new HostingError(400, "Confirm the monthly price and terms.");
}
