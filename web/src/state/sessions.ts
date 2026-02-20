import { signal } from "@preact/signals";
import type { SessionInfo } from "../api/types";

export type Theme = "glass" | "neon" | "minimal" | "tokyo-night" | "catppuccin" | "dracula";

export type ViewMode = "carousel" | "tiled" | "queue";

const validThemes: Theme[] = ["glass", "neon", "minimal", "tokyo-night", "catppuccin", "dracula"];
const rawTheme = localStorage.getItem("wsh-theme");
const storedTheme: Theme = validThemes.includes(rawTheme as Theme) ? (rawTheme as Theme) : "glass";
const storedAuthToken = localStorage.getItem("wsh-auth-token");
const storedZoom = parseFloat(localStorage.getItem("wsh-zoom") || "1");
const initialZoom = Number.isFinite(storedZoom) ? Math.max(0.5, Math.min(2.0, storedZoom)) : 1.0;

export const sessions = signal<string[]>([]);
export const focusedSession = signal<string | null>(null);
export const sessionOrder = signal<string[]>([]);
export const connectionState = signal<
  "connecting" | "connected" | "disconnected"
>("disconnected");
export const theme = signal<Theme>(storedTheme);
export const authToken = signal<string | null>(storedAuthToken);
export const authRequired = signal<boolean>(false);
export const authError = signal<string | null>(null);
export const zoomLevel = signal<number>(initialZoom);

export const sidebarWidth = signal<number>(
  parseFloat(localStorage.getItem("wsh-sidebar-width") || "15")
);
export const sidebarCollapsed = signal<boolean>(
  localStorage.getItem("wsh-sidebar-collapsed") === "true"
);

// Per-session info cache (tags, pid, command, etc.)
export const sessionInfoMap = signal<Map<string, SessionInfo>>(new Map());

export function setTheme(t: Theme): void {
  theme.value = t;
  localStorage.setItem("wsh-theme", t);
}

function setZoom(level: number): void {
  const clamped = Math.round(Math.max(0.5, Math.min(2.0, level)) * 10) / 10;
  zoomLevel.value = clamped;
  localStorage.setItem("wsh-zoom", String(clamped));
}

export function zoomIn(): void {
  setZoom(zoomLevel.value + 0.1);
}

export function zoomOut(): void {
  setZoom(zoomLevel.value - 0.1);
}

export function resetZoom(): void {
  setZoom(1.0);
}
