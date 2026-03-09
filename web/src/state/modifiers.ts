import { signal } from "@preact/signals";

export const ctrlActive = signal(false);
export const altActive = signal(false);

/** Activate ctrl, deactivating alt. */
export function toggleCtrl(): void {
  if (ctrlActive.value) {
    ctrlActive.value = false;
  } else {
    altActive.value = false;
    ctrlActive.value = true;
  }
}

/** Activate alt, deactivating ctrl. */
export function toggleAlt(): void {
  if (altActive.value) {
    altActive.value = false;
  } else {
    ctrlActive.value = false;
    altActive.value = true;
  }
}

/** Clear all modifiers. Called after a modified keypress is sent. */
export function clearModifiers(): void {
  ctrlActive.value = false;
  altActive.value = false;
}
