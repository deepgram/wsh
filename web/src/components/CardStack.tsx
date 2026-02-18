import { QueueCard } from "./QueueCard";
import type { QueueEntry } from "../queue/detector";
import type { WshClient } from "../api/ws";

const MAX_VISIBLE = 4;

interface CardStackProps {
  entries: QueueEntry[];
  client: WshClient;
  onDismiss: (sessionName: string) => void;
}

export function CardStack({ entries, client, onDismiss }: CardStackProps) {
  if (entries.length === 0) {
    return (
      <div class="card-stack-empty">
        <span class="card-stack-empty-text">All clear.</span>
      </div>
    );
  }

  const visible = entries.slice(0, MAX_VISIBLE);

  return (
    <div class="card-stack">
      {visible.map((entry, i) => (
        <div
          key={entry.id}
          class="card-stack-slot"
          style={{
            zIndex: MAX_VISIBLE - i,
            transform:
              i === 0 ? "none" : `translateY(${i * 4}px) scale(${1 - i * 0.02})`,
            opacity: i === 0 ? 1 : Math.max(0.3, 1 - i * 0.25),
            pointerEvents: i === 0 ? "auto" : "none",
          }}
        >
          <QueueCard
            entry={entry}
            client={client}
            onDismiss={onDismiss}
            interactive={i === 0}
          />
        </div>
      ))}
      {entries.length > 1 && (
        <div class="card-stack-count">{entries.length} items</div>
      )}
    </div>
  );
}
