import { signal, type Signal } from "@preact/signals";
import type { FormattedLine, Cursor, Overlay, Panel } from "../api/types";

export interface ScreenState {
  lines: FormattedLine[];
  cursor: Cursor;
  alternateActive: boolean;
  cols: number;
  rows: number;
  firstLineIndex: number;
  /** Total lines including scrollback (from server). */
  totalLines: number;
  /** Cached scrollback lines above the visible screen. */
  scrollbackLines: FormattedLine[];
  /** How far back we've loaded (offset into server scrollback). */
  scrollbackOffset: number;
  /** Whether all available scrollback has been fetched. */
  scrollbackComplete: boolean;
  /** Whether a scrollback fetch is currently in flight. */
  scrollbackLoading: boolean;
  overlays: Overlay[];
  panels: Panel[];
}

function makeEmptyScreen(): ScreenState {
  return {
    lines: [],
    cursor: { row: 0, col: 0, visible: true },
    alternateActive: false,
    cols: 80,
    rows: 24,
    firstLineIndex: 0,
    totalLines: 0,
    scrollbackLines: [],
    scrollbackOffset: 0,
    scrollbackComplete: false,
    scrollbackLoading: false,
    overlays: [],
    panels: [],
  };
}

// Per-session signals — each Terminal subscribes only to its own session
const screenSignals = new Map<string, Signal<ScreenState>>();

function getOrCreateSignal(session: string): Signal<ScreenState> {
  let s = screenSignals.get(session);
  if (!s) {
    s = signal<ScreenState>(makeEmptyScreen());
    screenSignals.set(session, s);
  }
  return s;
}

export function getScreenSignal(session: string): Signal<ScreenState> {
  return getOrCreateSignal(session);
}

export function getScreen(session: string): ScreenState {
  return getOrCreateSignal(session).value;
}

export function updateScreen(session: string, update: Partial<ScreenState>): void {
  const sig = getOrCreateSignal(session);
  sig.value = { ...sig.value, ...update };
}

export function setFullScreen(session: string, screen: ScreenState): void {
  const sig = getOrCreateSignal(session);
  sig.value = screen;
}

export function removeScreen(session: string): void {
  screenSignals.delete(session);
}

export function updateLine(
  session: string,
  index: number,
  line: FormattedLine,
): void {
  const sig = getOrCreateSignal(session);
  const current = sig.value;

  if (index >= 0 && index < current.rows) {
    const lines = [...current.lines];
    // Pad with empty lines if needed (handles appended lines)
    while (lines.length <= index) {
      lines.push("");
    }
    lines[index] = line;
    sig.value = { ...current, lines };
  }
}
