import { signal, effect } from "@preact/signals";
import { sessions } from "../state/sessions";
import { getScreenSignal, type ScreenState } from "../state/terminal";
import type { FormattedLine } from "../api/types";

export type TriggerType = "prompt" | "error" | "idle";

export interface QueueEntry {
  id: string;
  sessionName: string;
  trigger: TriggerType;
  triggerText: string;
  timestamp: number;
}

/** Reactive signal — UI subscribes to this for live updates. */
export const queueEntries = signal<QueueEntry[]>([]);

const IDLE_MS = 5000;
const SETTLE_MS = 300;

/** How many screen changes in the activity window count as a "burst." */
const BURST_THRESHOLD = 5;

/** How far back to count screen changes for burst detection (ms). */
const BURST_WINDOW_MS = 30_000;

const PROMPT_RE = [
  /\?\s*$/,
  /\[y\/n\]/i,
  /\(yes\/no\)/i,
  /password\s*:/i,
  /enter to continue/i,
  /press any key/i,
];

const ERROR_RE = [
  /\berror[:\[]/i,
  /\bFAILED\b/,
  /\bpanic[:(!\s]/i,
  /\bTraceback\b/,
  /\bfatal[:\s]/i,
];

function lineToText(line: FormattedLine): string {
  if (typeof line === "string") return line;
  return line.map((s) => s.text).join("");
}

function lineHasRed(line: FormattedLine): boolean {
  if (typeof line === "string") return false;
  return line.some((s) => {
    const idx = s.fg?.indexed;
    return idx === 1 || idx === 9;
  });
}

interface Tracker {
  gen: number;
  settleTimer: ReturnType<typeof setTimeout> | null;
  idleTimer: ReturnType<typeof setTimeout> | null;
  dispose: () => void;
  entryId: string | null;
  /** Timestamps of recent screen changes, for burst detection. */
  changeTimes: number[];
  /** Whether the initial effect has fired (skip the first one). */
  initialized: boolean;
}

/**
 * Watches all wsh sessions and surfaces ones needing human attention.
 *
 * Three detection strategies:
 * 1. Prompt — last non-empty line matches interactive prompt patterns
 * 2. Error  — bottom half of screen has red text or error keywords
 * 3. Idle   — screen goes quiet after a burst of activity
 *
 * For alternate-screen sessions (TUIs like Claude Code), idle only fires
 * after an activity burst (5+ screen changes in the last 30s), catching
 * the "tool finished working" pattern while ignoring always-quiet sessions.
 *
 * Call start() to begin watching, stop() to tear down.
 * Read queueEntries signal for the current queue state.
 */
export class QueueDetector {
  private trackers = new Map<string, Tracker>();
  private disposeWatcher: (() => void) | null = null;

  start(): void {
    this.disposeWatcher = effect(() => {
      const names = sessions.value;
      // Track new sessions
      for (const name of names) {
        if (!this.trackers.has(name)) this.track(name);
      }
      // Untrack removed sessions
      for (const [name] of this.trackers) {
        if (!names.includes(name)) this.untrack(name);
      }
    });
  }

  stop(): void {
    this.disposeWatcher?.();
    this.disposeWatcher = null;
    for (const name of [...this.trackers.keys()]) this.untrack(name);
    queueEntries.value = [];
  }

  dismiss(sessionName: string): void {
    const t = this.trackers.get(sessionName);
    if (!t) return;
    t.gen++;
    t.entryId = null;
    // Clear activity history so dismiss doesn't immediately re-trigger
    t.changeTimes = [];
    queueEntries.value = queueEntries.value.filter(
      (e) => e.sessionName !== sessionName,
    );
    // Restart idle timer so the same session can re-trigger later
    if (t.idleTimer) clearTimeout(t.idleTimer);
    t.idleTimer = setTimeout(() => this.onIdle(sessionName), IDLE_MS);
  }

  private track(name: string): void {
    const t: Tracker = {
      gen: 0,
      settleTimer: null,
      idleTimer: null,
      entryId: null,
      changeTimes: [],
      initialized: false,
      dispose: () => {},
    };
    this.trackers.set(name, t);

    // Subscribe to this session's screen signal
    const dispose = effect(() => {
      getScreenSignal(name).value; // read to subscribe
      this.onActivity(name);
    });
    t.dispose = dispose;
  }

  private untrack(name: string): void {
    const t = this.trackers.get(name);
    if (!t) return;
    t.dispose();
    if (t.settleTimer) clearTimeout(t.settleTimer);
    if (t.idleTimer) clearTimeout(t.idleTimer);
    this.trackers.delete(name);
    queueEntries.value = queueEntries.value.filter(
      (e) => e.sessionName !== name,
    );
  }

  private onActivity(name: string): void {
    const t = this.trackers.get(name);
    if (!t) return;

    // Skip the first effect fire — it's the initial read, not a real change
    if (!t.initialized) {
      t.initialized = true;
      // Still start the idle timer for normal-mode sessions
      t.idleTimer = setTimeout(() => this.onIdle(name), IDLE_MS);
      return;
    }

    // Record this change for burst detection
    const now = Date.now();
    t.changeTimes.push(now);
    // Prune old entries outside the burst window
    const cutoff = now - BURST_WINDOW_MS;
    t.changeTimes = t.changeTimes.filter((ts) => ts > cutoff);

    // Reset both timers on any screen change
    if (t.settleTimer) clearTimeout(t.settleTimer);
    if (t.idleTimer) clearTimeout(t.idleTimer);

    t.settleTimer = setTimeout(() => this.onSettle(name), SETTLE_MS);
    t.idleTimer = setTimeout(() => this.onIdle(name), IDLE_MS);
  }

  private onSettle(name: string): void {
    const t = this.trackers.get(name);
    if (!t || t.entryId) return;

    const screen = getScreenSignal(name).value;

    // Prompt detection — check last non-empty line
    // Works for both normal and alternate screen
    const promptText = this.findPrompt(screen);
    if (promptText !== null) {
      this.addEntry(name, t, "prompt", promptText);
      return;
    }

    // Error detection — check bottom half of screen
    const errorText = this.findError(screen);
    if (errorText !== null) {
      this.addEntry(name, t, "error", errorText);
    }
  }

  private onIdle(name: string): void {
    const t = this.trackers.get(name);
    if (!t || t.entryId) return;

    const screen = getScreenSignal(name).value;

    if (screen.alternateActive) {
      // Alternate screen (TUI): only trigger if there was a recent burst
      // of activity — meaning the tool was working and just stopped.
      const recentChanges = t.changeTimes.filter(
        (ts) => ts > Date.now() - BURST_WINDOW_MS,
      ).length;
      if (recentChanges < BURST_THRESHOLD) return;
      this.addEntry(name, t, "idle", "Waiting for input");
    } else {
      // Normal screen (shell): any idle after activity is meaningful
      this.addEntry(name, t, "idle", "Session idle");
    }
  }

  private addEntry(
    name: string,
    t: Tracker,
    trigger: TriggerType,
    text: string,
  ): void {
    const id = `${name}-${t.gen}`;
    t.entryId = id;
    queueEntries.value = [
      ...queueEntries.value,
      {
        id,
        sessionName: name,
        trigger,
        triggerText: text,
        timestamp: Date.now(),
      },
    ];
  }

  private findPrompt(screen: ScreenState): string | null {
    for (let i = screen.lines.length - 1; i >= 0; i--) {
      const text = lineToText(screen.lines[i]).trimEnd();
      if (!text) continue;
      for (const re of PROMPT_RE) {
        if (re.test(text)) return text;
      }
      return null; // only check the last non-empty line
    }
    return null;
  }

  private findError(screen: ScreenState): string | null {
    const start = Math.max(0, Math.floor(screen.lines.length / 2));
    for (let i = screen.lines.length - 1; i >= start; i--) {
      const line = screen.lines[i];
      const text = lineToText(line).trimEnd();
      if (!text) continue;

      if (lineHasRed(line)) return text;

      for (const re of ERROR_RE) {
        if (re.test(text)) return text;
      }
    }
    return null;
  }
}
