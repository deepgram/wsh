import { signal } from "@preact/signals";

export const ctrlActive = signal(false);
export const altActive = signal(false);

/** Toggle ctrl (independent of alt — both can be active). */
export function toggleCtrl(): void {
  ctrlActive.value = !ctrlActive.value;
}

/** Toggle alt (independent of ctrl — both can be active). */
export function toggleAlt(): void {
  altActive.value = !altActive.value;
}

/** Clear all modifiers. Called after a modified keypress is sent. */
export function clearModifiers(): void {
  ctrlActive.value = false;
  altActive.value = false;
}
