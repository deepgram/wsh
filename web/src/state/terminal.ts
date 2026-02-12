import { signal } from "@preact/signals";
import type { FormattedLine, Cursor } from "../api/types";

export interface ScreenState {
  lines: FormattedLine[];
  cursor: Cursor;
  alternateActive: boolean;
  cols: number;
  rows: number;
  firstLineIndex: number;
}

const emptyScreen: ScreenState = {
  lines: [],
  cursor: { row: 0, col: 0, visible: true },
  alternateActive: false,
  cols: 80,
  rows: 24,
  firstLineIndex: 0,
};

// Map of session name -> screen state
export const screens = signal<Map<string, ScreenState>>(new Map());

export function getScreen(session: string): ScreenState {
  return screens.value.get(session) ?? emptyScreen;
}

export function updateScreen(session: string, update: Partial<ScreenState>): void {
  const current = screens.value.get(session) ?? emptyScreen;
  const next = new Map(screens.value);
  next.set(session, { ...current, ...update });
  screens.value = next;
}

export function setFullScreen(session: string, screen: ScreenState): void {
  const next = new Map(screens.value);
  next.set(session, screen);
  screens.value = next;
}

export function updateLine(
  session: string,
  index: number,
  line: FormattedLine,
): void {
  const current = screens.value.get(session) ?? emptyScreen;

  // index is relative to the visible screen (screen row 0 = first visible line)
  if (index >= 0 && index < current.rows && index < current.lines.length) {
    const lines = [...current.lines];
    lines[index] = line;
    const next = new Map(screens.value);
    next.set(session, { ...current, lines });
    screens.value = next;
  }
}
