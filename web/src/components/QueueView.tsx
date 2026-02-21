import { useCallback, useEffect, useRef, useState } from "preact/hooks";
import type { WshClient } from "../api/ws";
import { idleQueues, enqueueSession, dismissQueueEntry, sessionStatuses } from "../state/groups";
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
  const pending = queue.filter((e) => e.status === "pending");
  const handled = queue.filter((e) => e.status === "handled");

  // Sessions not in the queue at all (active, haven't become idle yet)
  const queuedNames = new Set(queue.map((e) => e.session));
  const active = sessions.filter((s) => !queuedNames.has(s));

  // Manual selection overrides automatic queue order
  const [manualSelection, setManualSelection] = useState<string | null>(null);

  // Current session to display (manual override or first pending)
  const autoSession = pending[0]?.session || null;
  const currentSession = manualSelection && sessions.includes(manualSelection)
    ? manualSelection
    : autoSession;

  // Focus the current queued session
  useEffect(() => {
    if (currentSession) {
      focusedSession.value = currentSession;
    }
  }, [currentSession]);

  // Watch sessionStatuses for idle transitions and enqueue sessions
  const prevStatuses = useRef<Map<string, string>>(new Map());
  useEffect(() => {
    const statuses = sessionStatuses.value;
    for (const s of sessions) {
      const current = statuses.get(s);
      const prev = prevStatuses.current.get(s);
      if (current === "idle" && prev !== "idle") {
        enqueueSession(groupTag, s);
      }
    }
    // Update previous statuses
    const updated = new Map<string, string>();
    for (const s of sessions) {
      const st = statuses.get(s);
      if (st) updated.set(s, st);
    }
    prevStatuses.current = updated;
  }, [sessions, groupTag, sessionStatuses.value]);

  // Dismiss current session
  const handleDismiss = useCallback(() => {
    if (!currentSession) return;
    const isPending = pending.some((e) => e.session === currentSession);
    if (isPending) {
      dismissQueueEntry(groupTag, currentSession);
    }
    setManualSelection(null);
  }, [groupTag, currentSession, pending]);

  // Ctrl+Shift+Enter to dismiss
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.shiftKey && !e.altKey && !e.metaKey && e.key === "Enter") {
        e.preventDefault();
        handleDismiss();
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [handleDismiss]);

  return (
    <div class="queue-view">
      {/* Top bar */}
      <div class="queue-top-bar">
        <div class="queue-pending">
          <div class="queue-section-header">
            <span class="queue-section-label">Idle ({pending.length})</span>
            {currentSession && pending.some((e) => e.session === currentSession) && (
              <kbd class="queue-shortcut-hint">Ctrl+Shift+Enter to dismiss</kbd>
            )}
          </div>
          <div class="queue-thumbnails">
            {pending.map((e) => (
              <div
                key={e.session}
                class={`queue-thumb ${e.session === currentSession ? "active" : ""}`}
                onClick={() => setManualSelection(e.session)}
              >
                <MiniTermContent session={e.session} />
              </div>
            ))}
          </div>
        </div>
        <div class="queue-handled">
          <span class="queue-section-label">Running ({active.length + handled.length})</span>
          <div class="queue-thumbnails muted">
            {active.map((s) => (
              <div key={s} class={`queue-thumb ${s === currentSession ? "active" : ""}`}
                onClick={() => setManualSelection(s)}>
                <MiniTermContent session={s} />
              </div>
            ))}
            {handled.map((e) => (
              <div key={e.session} class={`queue-thumb handled ${e.session === currentSession ? "active" : ""}`}
                onClick={() => setManualSelection(e.session)}>
                <MiniTermContent session={e.session} />
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
