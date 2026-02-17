import {
  sessionOrder,
  focusedSession,
  viewMode,
  tileLayout,
  tileSelection,
  toggleTileSelection,
  clearTileSelection,
} from "../state/sessions";
import { SessionThumbnail } from "./SessionThumbnail";
import type { WshClient } from "../api/ws";

interface SessionGridProps {
  client: WshClient;
}

export function SessionGrid({ client }: SessionGridProps) {
  const order = sessionOrder.value;
  const selection = tileSelection.value;

  const handleSelect = (name: string) => {
    focusedSession.value = name;
    clearTileSelection();
    viewMode.value = "focused";
  };

  const handleCreate = async () => {
    try {
      await client.createSession();
    } catch (e) {
      console.error("Failed to create session:", e);
    }
  };

  const handleClose = async (name: string) => {
    try {
      await client.killSession(name);
    } catch (e) {
      console.error("Failed to kill session:", e);
    }
  };

  const handleTileSelected = (e: Event) => {
    e.stopPropagation();
    if (selection.length < 2) return;
    const evenSize = 1 / selection.length;
    tileLayout.value = {
      sessions: [...selection],
      sizes: selection.map(() => evenSize),
    };
    clearTileSelection();
    viewMode.value = "tiled";
  };

  return (
    <div
      class="session-grid-backdrop"
      onClick={() => {
        clearTileSelection();
        viewMode.value = "focused";
      }}
    >
      <div class="session-grid" onClick={(e) => e.stopPropagation()}>
        {order.map((name) => (
          <SessionThumbnail
            key={name}
            session={name}
            focused={name === focusedSession.value}
            selectionIndex={selection.indexOf(name)}
            onSelect={() => handleSelect(name)}
            onClose={() => handleClose(name)}
            onToggleTile={() => toggleTileSelection(name)}
          />
        ))}
        <div class="session-thumbnail new-session" onClick={handleCreate}>
          <span class="new-session-icon">+</span>
        </div>
      </div>

      {selection.length >= 2 && (
        <button class="tile-action-btn" onClick={handleTileSelected}>
          Tile {selection.length} sessions
        </button>
      )}
    </div>
  );
}
