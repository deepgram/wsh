import { signal } from "@preact/signals";

export type Theme = "glass" | "neon" | "minimal";

const storedTheme = (localStorage.getItem("wsh-theme") as Theme) || "glass";

export const sessions = signal<string[]>([]);
export const focusedSession = signal<string | null>(null);
export const sessionOrder = signal<string[]>([]);
export const viewMode = signal<"focused" | "overview" | "tiled">("focused");
export const tileLayout = signal<{
  sessions: string[];
  sizes: number[];
} | null>(null);
export const connectionState = signal<
  "connecting" | "connected" | "disconnected"
>("disconnected");
export const theme = signal<Theme>(storedTheme);

export function cycleTheme(): Theme {
  const order: Theme[] = ["glass", "neon", "minimal"];
  const idx = order.indexOf(theme.value);
  const next = order[(idx + 1) % order.length];
  theme.value = next;
  localStorage.setItem("wsh-theme", next);
  return next;
}
