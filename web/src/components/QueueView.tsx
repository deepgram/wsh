import { useCallback, useEffect, useState } from "preact/hooks";
import type { WshClient } from "../api/ws";
import { quiescenceQueues, enqueueSession, dismissQueueEntry } from "../state/groups";
import { focusedSession } from "../state/sessions";
import { SessionPane } from "./SessionPane";
import { MiniTermContent } from "./MiniViewPreview";

interface QueueViewProps {
  sessions: string[];
  groupTag: string;
  client: WshClient;
}

export function QueueView({ sessions, groupTag, client }: QueueViewProps) {
  const queue = quiescenceQueues.value[groupTag] || [];
  const pending = queue.filter((e) => e.status === "pending");
  const handled = queue.filter((e) => e.status === "handled");

  // Sessions not in the queue at all (active, haven't become quiescent yet)
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

  // Start quiescence watching for all sessions in this group
  useEffect(() => {
    const controllers: AbortController[] = [];

    const watchSession = (sessionName: string) => {
      const controller = new AbortController();
      controllers.push(controller);

      const doWatch = async () => {
        while (!controller.signal.aborted) {
          try {
            await client.awaitQuiesce(sessionName, 300);
            if (controller.signal.aborted) return;
            enqueueSession(groupTag, sessionName);
            // After enqueuing, stop watching (will re-watch on dismiss)
            return;
          } catch {
            // Timeout or error -- retry
            if (controller.signal.aborted) return;
          }
        }
      };
      doWatch();
    };

    // Watch sessions that aren't already pending
    const pendingNames = new Set(pending.map((e) => e.session));
    for (const s of sessions) {
      if (!pendingNames.has(s)) {
        watchSession(s);
      }
    }

    return () => {
      for (const c of controllers) c.abort();
    };
  }, [sessions, groupTag, client, pending.length]);

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
          <span class="queue-section-label">Pending ({pending.length})</span>
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
          <span class="queue-section-label">Active ({active.length + handled.length})</span>
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
          <div class="queue-dismiss-bar">
            <button class="queue-dismiss-btn" onClick={handleDismiss} title="Dismiss (Ctrl+Shift+Enter)">
              &#10003; Done
            </button>
          </div>
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
