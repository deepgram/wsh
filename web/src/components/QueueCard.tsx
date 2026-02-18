import { useRef } from "preact/hooks";
import { Terminal } from "./Terminal";
import { InputBar } from "./InputBar";
import type { WshClient } from "../api/ws";
import type { QueueEntry } from "../api/orchestrator";

interface QueueCardProps {
  entry: QueueEntry;
  wshClient: WshClient;
  onResolve: (entryId: string, action: string, text?: string) => void;
  style?: Record<string, string>;
}

function cardTypeLabel(entry: QueueEntry): string {
  if (entry.kind === "approval_needed") return "Approval";
  if (entry.kind === "error") return "Error";
  return "Attention";
}

function cardTypeClass(entry: QueueEntry): string {
  if (entry.kind === "approval_needed") return "card-approval";
  if (entry.kind === "error") return "card-error";
  return "card-attention";
}

function formatTimestamp(ts: string): string {
  try {
    const date = new Date(ts);
    return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  } catch {
    return ts;
  }
}

export function QueueCard({ entry, wshClient, onResolve, style }: QueueCardProps) {
  const textInputRef = useRef<HTMLInputElement>(null);

  const handleTextSend = () => {
    const input = textInputRef.current;
    if (!input || !input.value.trim()) return;
    const text = input.value.trim();
    onResolve(entry.id, "respond", text);
    // Also send to the wsh session
    wshClient.sendInput(entry.session_name, text + "\n").catch((e) => {
      console.error("Failed to send input:", e);
    });
    input.value = "";
  };

  const handleApprove = () => {
    onResolve(entry.id, "approve");
    wshClient.sendInput(entry.session_name, "y\n").catch(() => {});
  };

  const handleReject = () => {
    onResolve(entry.id, "reject");
    wshClient.sendInput(entry.session_name, "n\n").catch(() => {});
  };

  const handleAcknowledge = () => {
    onResolve(entry.id, "acknowledge");
  };

  const handleResolved = () => {
    onResolve(entry.id, "resolved");
  };

  return (
    <div class={`queue-card ${cardTypeClass(entry)}`} style={style}>
      <div class="queue-card-header">
        <span class="card-type-badge">{cardTypeLabel(entry)}</span>
        <span class="card-session-name">{entry.session_name}</span>
        <span class="card-project">{entry.project_id}</span>
        <span class="card-timestamp">{formatTimestamp(entry.ts)}</span>
      </div>

      <div class="queue-card-terminal">
        <Terminal session={entry.session_name} />
        <InputBar session={entry.session_name} client={wshClient} />
      </div>

      <div class="queue-card-actions">
        <div class="card-event-text">{entry.text}</div>

        <div class="card-buttons">
          {entry.kind === "approval_needed" && (
            <>
              <button class="card-btn card-btn-approve" onClick={handleApprove}>
                Approve
              </button>
              <button class="card-btn card-btn-reject" onClick={handleReject}>
                Reject
              </button>
            </>
          )}
          {entry.kind === "error" && (
            <button class="card-btn card-btn-resolve" onClick={handleResolved}>
              Resolved
            </button>
          )}
          {entry.kind !== "approval_needed" && entry.kind !== "error" && (
            <button class="card-btn card-btn-ack" onClick={handleAcknowledge}>
              Acknowledged
            </button>
          )}
        </div>

        <div class="card-text-input">
          <input
            ref={textInputRef}
            type="text"
            placeholder="Type a response..."
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                handleTextSend();
              }
            }}
          />
          <button class="card-btn card-btn-send" onClick={handleTextSend}>
            Send
          </button>
        </div>
      </div>
    </div>
  );
}
