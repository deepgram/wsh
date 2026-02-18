import { QueueCard } from "./QueueCard";
import type { WshClient } from "../api/ws";
import type { QueueEntry } from "../api/orchestrator";

interface CardStackProps {
  entries: QueueEntry[];
  wshClient: WshClient;
  onResolve: (entryId: string, action: string, text?: string) => void;
}

export function CardStack({ entries, wshClient, onResolve }: CardStackProps) {
  if (entries.length === 0) {
    return (
      <div class="card-stack-empty">
        <div class="empty-check">&#10003;</div>
        <div class="empty-text">All clear</div>
        <div class="empty-subtext">No sessions need your attention</div>
      </div>
    );
  }

  return (
    <div class="card-stack">
      {entries.map((entry, i) => {
        // Stack offset: first card is on top, subsequent peek behind
        const isActive = i === 0;
        const peekOffset = Math.min(i, 4) * 4; // px offset per card, max 4 visible
        const scale = isActive ? 1 : 1 - i * 0.02;
        const zIndex = entries.length - i;
        const opacity = i > 3 ? 0 : 1;

        const style: Record<string, string> = {
          transform: `translateY(${peekOffset}px) scale(${scale})`,
          zIndex: String(zIndex),
          opacity: String(opacity),
          pointerEvents: isActive ? "auto" : "none",
        };

        return (
          <QueueCard
            key={entry.id}
            entry={entry}
            wshClient={wshClient}
            onResolve={onResolve}
            style={style}
          />
        );
      })}
    </div>
  );
}
