import { useState, type FocusEvent, type SyntheticEvent } from "react";
type Field = HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement;
const isField = (target: EventTarget): target is Field =>
  target instanceof HTMLInputElement || target instanceof HTMLSelectElement || target instanceof HTMLTextAreaElement;
/**
 * Live checks on top of native constraint validation: a field reports after it was first left
 * (or on submit) and then updates while typing. Messages come from the browser in its language.
 */
export function useLiveValidation() {
  const [errors, setErrors] = useState<Record<string, string>>({});
  const report = (target: EventTarget, touched: boolean) => {
    if (!isField(target) || !target.name) return;
    const message = target.validity.valid ? "" : target.validationMessage;
    setErrors((current) => (touched || target.name in current) && current[target.name] !== message ? { ...current, [target.name]: message } : current);
  };
  return {
    errors,
    form: {
      onBlur: (event: FocusEvent<HTMLFormElement>) => { report(event.target, true); },
      onInput: (event: SyntheticEvent<HTMLFormElement>) => { report(event.target, false); },
      onInvalidCapture: (event: SyntheticEvent<HTMLFormElement>) => { report(event.target, true); },
    },
  };
}
/** Rendered after its label (not inside it) so the message never becomes part of the field's name. */
export function FieldError({ message, className = "-mt-3" }: { message?: string | undefined; className?: string }) {
  return message ? <span className={`${className} text-xs text-danger`} role="alert">{message}</span> : null;
}
