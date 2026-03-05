import { signal } from "@preact/signals";
import type { WshClient } from "../api/ws";
import { sessionInfoMap } from "../state/sessions";

export interface DragState {
  type: "session";
  sessionName: string;
  shiftHeld: boolean;
}

export const dragState = signal<DragState | null>(null);
export const dropTargetTag = signal<string | null>(null);

export function startSessionDrag(sessionName: string, e: DragEvent): void {
  dragState.value = {
    type: "session",
    sessionName,
    shiftHeld: e.shiftKey,
  };
  if (e.dataTransfer) {
    e.dataTransfer.effectAllowed = "move";
    e.dataTransfer.setData("text/plain", sessionName);
  }
}

export function updateShiftState(e: DragEvent): void {
  const current = dragState.value;
  if (current) {
    dragState.value = { ...current, shiftHeld: e.shiftKey };
  }
}

export function handleGroupDragOver(tag: string, e: DragEvent): void {
  e.preventDefault();
  if (e.dataTransfer) {
    e.dataTransfer.dropEffect = "move";
  }
  updateShiftState(e);
  // Don't allow drop on "all" or "untagged" groups — they are virtual
  if (tag === "all" || tag === "untagged") {
    dropTargetTag.value = null;
    return;
  }
  dropTargetTag.value = tag;
}

export function handleGroupDragLeave(): void {
  dropTargetTag.value = null;
}

export function handleGroupDrop(targetTag: string, e: DragEvent, client: WshClient): void {
  e.preventDefault();
  const state = dragState.value;
  dropTargetTag.value = null;
  dragState.value = null;

  if (!state || state.type !== "session") return;
  if (targetTag === "all" || targetTag === "untagged") return;

  const sessionName = state.sessionName;
  const info = sessionInfoMap.value.get(sessionName);
  if (!info) return;

  if (state.shiftHeld) {
    // Add mode: keep existing tags, add new one
    if (!info.tags.includes(targetTag)) {
      client.updateTags(sessionName, [targetTag]).catch((err) => {
        console.error("Failed to add tag:", err);
      });
    }
  } else {
    // Move mode: remove all existing tags, add target tag
    const tagsToRemove = info.tags.filter((t) => t !== targetTag);
    const tagsToAdd = info.tags.includes(targetTag) ? [] : [targetTag];
    client.updateTags(sessionName, tagsToAdd, tagsToRemove).catch((err) => {
      console.error("Failed to move session:", err);
    });
  }
}

export function endDrag(): void {
  dragState.value = null;
  dropTargetTag.value = null;
}
