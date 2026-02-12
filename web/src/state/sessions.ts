import { signal } from "@preact/signals";

export const sessions = signal<string[]>([]);
export const activeSession = signal<string | null>(null);
export const connectionState = signal<
  "connecting" | "connected" | "disconnected"
>("disconnected");
