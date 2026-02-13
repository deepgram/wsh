import {
  sessionOrder,
  focusedSession,
  viewMode,
  tileLayout,
} from "../state/sessions";
import { SessionThumbnail } from "./SessionThumbnail";
import type { WshClient } from "../api/ws";

interface SessionGridProps {
  client: WshClient;
}

export function SessionGrid({ client }: SessionGridProps) {
  const order = sessionOrder.value;

  const handleSelect = (name: string) => {
    focusedSession.value = name;
    viewMode.value = "focused";
  };

  const handleCreate = async () => {
    try {
      await client.createSession();
      // session_created lifecycle event will add it to the list
    } catch (e) {
      console.error("Failed to create session:", e);
    }
  };

  const handleClose = async (name: string) => {
    try {
      await client.killSession(name);
      // session_destroyed lifecycle event will remove it
    } catch (e) {
      console.error("Failed to kill session:", e);
    }
  };

  const handleTile = (name: string) => {
    // Only allow tiling on wide screens
    if (window.innerWidth < 768) return;
    const current = focusedSession.value;
    const sessionsForTile: string[] = [];
    if (current && current !== name) {
      sessionsForTile.push(current, name);
    } else {
      sessionsForTile.push(name);
      const other = order.find((s) => s !== name);
      if (other) sessionsForTile.push(other);
    }
    if (sessionsForTile.length >= 2) {
      tileLayout.value = {
        sessions: sessionsForTile.slice(0, 2),
        sizes: [0.5, 0.5],
      };
      viewMode.value = "tiled";
    }
  };

  const canTile = order.length >= 2 && window.innerWidth >= 768;

  return (
    <div
      class="session-grid-backdrop"
      onClick={() => {
        viewMode.value = "focused";
      }}
    >
      <div class="session-grid" onClick={(e) => e.stopPropagation()}>
        {order.map((name) => (
          <SessionThumbnail
            key={name}
            session={name}
            focused={name === focusedSession.value}
            onSelect={() => handleSelect(name)}
            onClose={() => handleClose(name)}
            onTile={canTile ? () => handleTile(name) : undefined}
          />
        ))}
        <div class="session-thumbnail new-session" onClick={handleCreate}>
          <span class="new-session-icon">+</span>
        </div>
      </div>
    </div>
  );
}
