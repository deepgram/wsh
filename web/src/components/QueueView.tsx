import { useCallback, useEffect } from "preact/hooks";
import type { WshClient } from "../api/ws";
import { quiescenceQueues, enqueueSession, dismissQueueEntry } from "../state/groups";
import { focusedSession } from "../state/sessions";
import { SessionPane } from "./SessionPane";
import { MiniTerminal } from "./MiniTerminal";

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

  // Current session to display (first pending)
  const currentEntry = pending[0] || null;
  const currentSession = currentEntry?.session || null;

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
    dismissQueueEntry(groupTag, currentSession);

    // Re-subscribe to quiescence for the dismissed session
    // (it will be picked up by the effect above on re-render)
  }, [groupTag, currentSession]);

  // Super+Enter to dismiss
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if ((e.metaKey || (e.ctrlKey && e.shiftKey)) && e.key === "Enter") {
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
                onClick={() => { focusedSession.value = e.session; }}
              >
                <MiniTerminal session={e.session} />
              </div>
            ))}
          </div>
        </div>
        <div class="queue-handled">
          <span class="queue-section-label">Active ({active.length + handled.length})</span>
          <div class="queue-thumbnails muted">
            {active.map((s) => (
              <div key={s} class="queue-thumb">
                <MiniTerminal session={s} />
              </div>
            ))}
            {handled.map((e) => (
              <div key={e.session} class="queue-thumb handled">
                <MiniTerminal session={e.session} />
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
            <button class="queue-dismiss-btn" onClick={handleDismiss} title="Dismiss (Super+Enter)">
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
