import { useCallback, useEffect, useRef, useState } from "preact/hooks";
import type { WshClient } from "../api/ws";
import { idleQueues, enqueueSession, dismissQueueEntry, removeQueueEntry, sessionStatuses } from "../state/groups";
import { focusedSession } from "../state/sessions";
import { SessionPane } from "./SessionPane";
import { MiniTermContent } from "./MiniViewPreview";

interface QueueViewProps {
  sessions: string[];
  groupTag: string;
  client: WshClient;
}

export function QueueView({ sessions, groupTag, client }: QueueViewProps) {
  const queue = idleQueues.value[groupTag] || [];
  const statuses = sessionStatuses.value;

  // Idle section: pending first (by idleAt), then acknowledged (by idleAt)
  const pending = queue
    .filter((e) => e.status === "pending")
    .sort((a, b) => a.idleAt - b.idleAt);
  const acknowledged = queue
    .filter((e) => e.status === "acknowledged")
    .sort((a, b) => a.idleAt - b.idleAt);
  const idle = [...pending, ...acknowledged];

  // Running section: sessions whose actual status is not idle
  const idleNames = new Set(queue.map((e) => e.session));
  const running = sessions.filter(
    (s) => !idleNames.has(s) && statuses.get(s) !== "idle"
  );

  // Flat navigation list: idle then running
  const navList = [...idle.map((e) => e.session), ...running];

  // Selection state
  const [selectedSession, setSelectedSession] = useState<string | null>(null);

  // Resolve current session: manual selection if valid, else oldest pending, else "All caught up"
  const oldestPending = pending[0]?.session || null;
  const currentSession =
    selectedSession && navList.includes(selectedSession)
      ? selectedSession
      : oldestPending;

  // Focus the current session for other components
  useEffect(() => {
    if (currentSession) {
      focusedSession.value = currentSession;
    }
  }, [currentSession]);

  // Watch sessionStatuses for transitions
  const prevStatuses = useRef<Map<string, string>>(new Map());
  useEffect(() => {
    const statuses = sessionStatuses.value;
    for (const s of sessions) {
      const current = statuses.get(s);
      const prev = prevStatuses.current.get(s);
      if (current === "idle" && prev !== "idle") {
        enqueueSession(groupTag, s);
      } else if (current !== "idle" && prev === "idle") {
        removeQueueEntry(groupTag, s);
        // Pin the currently viewed session so it doesn't vanish when it
        // transitions to running (e.g. user typed into it). It moves to
        // the running section visually but stays selected.
        if (s === currentSession) {
          setSelectedSession(s);
        }
      }
    }
    const updated = new Map<string, string>();
    for (const s of sessions) {
      const st = statuses.get(s);
      if (st) updated.set(s, st);
    }
    prevStatuses.current = updated;
  }, [sessions, groupTag, sessionStatuses.value]);

  // Dismiss: acknowledge current if pending, then jump to next pending
  const handleDismiss = useCallback(() => {
    if (currentSession) {
      const isPending = pending.some((e) => e.session === currentSession);
      if (isPending) {
        dismissQueueEntry(groupTag, currentSession);
      }
    }
    // Jump to oldest pending (after the one we just dismissed)
    // The signal update is synchronous, so re-read the queue
    const updatedQueue = idleQueues.value[groupTag] || [];
    const nextPending = updatedQueue.find((e) => e.status === "pending" && e.session !== currentSession);
    setSelectedSession(nextPending?.session || null);
  }, [groupTag, currentSession, pending]);

  // Left/right navigation: Ctrl+Shift+H/L or Left/Right
  const navigate = useCallback((direction: -1 | 1) => {
    if (navList.length === 0) return;
    const currentIndex = currentSession ? navList.indexOf(currentSession) : -1;
    const newIndex = currentIndex === -1
      ? 0
      : (currentIndex + direction + navList.length) % navList.length;
    setSelectedSession(navList[newIndex]);
  }, [navList, currentSession]);

  // Keep refs to latest callbacks so the keyboard handler (registered once)
  // always calls the current version without re-registering on every render.
  const navigateRef = useRef(navigate);
  navigateRef.current = navigate;
  const handleDismissRef = useRef(handleDismiss);
  handleDismissRef.current = handleDismiss;

  // Keyboard handler — registered once, uses refs for latest callbacks
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (!e.ctrlKey || !e.shiftKey || e.altKey || e.metaKey) return;

      if (e.key === "ArrowLeft" || e.key === "h" || e.key === "H") {
        e.preventDefault();
        navigateRef.current(-1);
      } else if (e.key === "ArrowRight" || e.key === "l" || e.key === "L") {
        e.preventDefault();
        navigateRef.current(1);
      } else if (e.key === "Enter") {
        e.preventDefault();
        handleDismissRef.current();
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  // Idle section label with pending count
  const idleLabel = pending.length > 0
    ? `Idle (${pending.length} new \u00b7 ${idle.length})`
    : `Idle (${idle.length})`;

  return (
    <div class="queue-view">
      {/* Top bar */}
      <div class="queue-top-bar">
        <div class="queue-pending">
          <div class="queue-section-header">
            <span class="queue-section-label">{idleLabel}</span>
            <kbd class="queue-shortcut-hint">Ctrl+Shift+Enter to dismiss</kbd>
          </div>
          <div class="queue-thumbnails">
            {idle.map((e) => (
              <div
                key={e.session}
                class={`queue-thumb${e.session === currentSession ? " active" : ""}${e.status === "pending" ? " pending" : ""}`}
                onClick={() => setSelectedSession(e.session)}
              >
                {e.status === "pending" && <span class="queue-pending-dot" />}
                <MiniTermContent session={e.session} />
              </div>
            ))}
          </div>
        </div>
        <div class="queue-handled">
          <span class="queue-section-label">Running ({running.length})</span>
          <div class="queue-thumbnails">
            {running.map((s) => (
              <div
                key={s}
                class={`queue-thumb${s === currentSession ? " active" : ""}`}
                onClick={() => setSelectedSession(s)}
              >
                <MiniTermContent session={s} />
              </div>
            ))}
          </div>
        </div>
      </div>

      {/* Center content */}
      {currentSession ? (
        <div class="queue-center">
          <SessionPane session={currentSession} client={client} />
        </div>
      ) : (
        <div class="queue-empty">
          <div class="queue-empty-icon">&#10003;</div>
          <div class="queue-empty-text">All caught up</div>
        </div>
      )}
    </div>
  );
}
