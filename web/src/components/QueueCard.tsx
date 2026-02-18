import { Terminal } from "./Terminal";
import { InputBar } from "./InputBar";
import type { QueueEntry, TriggerType } from "../queue/detector";
import type { WshClient } from "../api/ws";

function triggerBadge(trigger: TriggerType): string {
  switch (trigger) {
    case "prompt":
      return "prompt";
    case "error":
      return "error";
    case "idle":
      return "idle";
  }
}

function relativeTime(ts: number): string {
  const seconds = Math.floor((Date.now() - ts) / 1000);
  if (seconds < 5) return "just now";
  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  return `${Math.floor(minutes / 60)}h ago`;
}

interface QueueCardProps {
  entry: QueueEntry;
  client: WshClient;
  onDismiss: (sessionName: string) => void;
  interactive: boolean;
}

export function QueueCard({
  entry,
  client,
  onDismiss,
  interactive,
}: QueueCardProps) {
  return (
    <div class={`queue-card ${interactive ? "" : "queue-card-behind"}`}>
      <div class="queue-card-header">
        <span class="queue-card-session">{entry.sessionName}</span>
        <span class={`queue-card-badge queue-card-badge-${entry.trigger}`}>
          {triggerBadge(entry.trigger)}
        </span>
        <span class="queue-card-time">{relativeTime(entry.timestamp)}</span>
      </div>

      <div class="queue-card-terminal">
        <Terminal session={entry.sessionName} client={client} />
      </div>

      {interactive && (
        <>
          <InputBar session={entry.sessionName} client={client} />
          <div class="queue-card-actions">
            <span class="queue-card-trigger-text" title={entry.triggerText}>
              {entry.triggerText}
            </span>
            <button
              class="queue-card-dismiss"
              onClick={() => onDismiss(entry.sessionName)}
            >
              Dismiss
            </button>
          </div>
        </>
      )}
    </div>
  );
}
