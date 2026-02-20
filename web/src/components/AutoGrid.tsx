import { useCallback, useEffect, useState, useMemo } from "preact/hooks";
import type { WshClient } from "../api/ws";
import { focusedSession } from "../state/sessions";
import { setViewModeForGroup, selectedGroups } from "../state/groups";
import { SessionPane } from "./SessionPane";

interface AutoGridProps {
  sessions: string[];
  client: WshClient;
}

/** Compute the best NxM grid layout. Returns an array of rows, each with a count of cells. */
function computeGridLayout(count: number): { count: number }[] {
  if (count <= 0) return [];
  if (count === 1) return [{ count: 1 }];

  const cols = Math.ceil(Math.sqrt(count));
  const fullRows = Math.floor(count / cols);
  const remainder = count % cols;

  const rows: { count: number }[] = [];
  for (let i = 0; i < fullRows; i++) {
    rows.push({ count: cols });
  }
  if (remainder > 0) {
    rows.push({ count: remainder });
  }
  return rows;
}

export function AutoGrid({ sessions, client }: AutoGridProps) {
  const focused = focusedSession.value;
  // Local session order for drag-to-swap (starts matching sessions prop)
  const [localOrder, setLocalOrder] = useState<string[]>(sessions);
  const [dragSource, setDragSource] = useState<string | null>(null);
  const [dragTarget, setDragTarget] = useState<string | null>(null);

  // Keep localOrder in sync with sessions prop (new sessions added, removed sessions dropped)
  const orderedSessions = useMemo(() => {
    const existing = localOrder.filter((s) => sessions.includes(s));
    const added = sessions.filter((s) => !localOrder.includes(s));
    const result = [...existing, ...added];
    if (result.join(",") !== localOrder.join(",")) {
      setLocalOrder(result);
    }
    return result;
  }, [sessions, localOrder]);

  const layout = computeGridLayout(orderedSessions.length);

  const handleClick = useCallback((session: string) => {
    focusedSession.value = session;
  }, []);

  const handleDoubleClick = useCallback((session: string) => {
    // Switch to carousel mode focused on this session
    focusedSession.value = session;
    const primaryTag = selectedGroups.value[0] || "all";
    setViewModeForGroup(primaryTag, "carousel");
  }, []);

  // Drag-to-swap handlers
  const handleDragStart = useCallback((session: string, e: DragEvent) => {
    setDragSource(session);
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", session);
    }
  }, []);

  const handleDragOver = useCallback((session: string, e: DragEvent) => {
    e.preventDefault();
    if (e.dataTransfer) {
      e.dataTransfer.dropEffect = "move";
    }
    setDragTarget(session);
  }, []);

  const handleDragLeave = useCallback(() => {
    setDragTarget(null);
  }, []);

  const handleDrop = useCallback((targetSession: string, e: DragEvent) => {
    e.preventDefault();
    setDragTarget(null);
    if (!dragSource || dragSource === targetSession) {
      setDragSource(null);
      return;
    }
    // Swap positions
    setLocalOrder((prev) => {
      const newOrder = [...prev];
      const srcIdx = newOrder.indexOf(dragSource);
      const tgtIdx = newOrder.indexOf(targetSession);
      if (srcIdx >= 0 && tgtIdx >= 0) {
        [newOrder[srcIdx], newOrder[tgtIdx]] = [newOrder[tgtIdx], newOrder[srcIdx]];
      }
      return newOrder;
    });
    setDragSource(null);
  }, [dragSource]);

  const handleDragEnd = useCallback(() => {
    setDragSource(null);
    setDragTarget(null);
  }, []);

  // Keyboard navigation: Ctrl+Shift+Arrows or Ctrl+Shift+HJKL to move focus between cells
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (!e.ctrlKey || !e.shiftKey || e.altKey || e.metaKey) return;
      if (!["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "h", "j", "k", "l", "H", "J", "K", "L"].includes(e.key)) return;

      e.preventDefault();

      const currentFocused = focusedSession.value;
      const currentIdx = orderedSessions.indexOf(currentFocused ?? "");
      if (currentIdx < 0 && orderedSessions.length > 0) {
        focusedSession.value = orderedSessions[0];
        return;
      }

      const cols = layout.length > 0 ? layout[0].count : 1;
      const row = Math.floor(currentIdx / cols);
      const col = currentIdx % cols;

      let newRow = row;
      let newCol = col;

      switch (e.key) {
        case "ArrowLeft":
        case "h":
        case "H":
          newCol = Math.max(0, col - 1);
          break;
        case "ArrowRight":
        case "l":
        case "L":
          newCol = Math.min(cols - 1, col + 1);
          break;
        case "ArrowUp":
        case "k":
        case "K":
          newRow = Math.max(0, row - 1);
          break;
        case "ArrowDown":
        case "j":
        case "J":
          newRow = Math.min(layout.length - 1, row + 1);
          break;
      }

      // Clamp column to actual cells in the target row
      const rowCellCount = layout[newRow]?.count ?? cols;
      newCol = Math.min(newCol, rowCellCount - 1);

      const newIdx = newRow * cols + newCol;
      if (newIdx >= 0 && newIdx < orderedSessions.length) {
        focusedSession.value = orderedSessions[newIdx];
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [orderedSessions, layout]);

  if (orderedSessions.length === 0) return null;

  // Build the grid rows
  let sessionIdx = 0;
  return (
    <div class="auto-grid">
      {layout.map((row, rowIdx) => (
        <div key={rowIdx} class="auto-grid-row">
          {Array.from({ length: row.count }, (_, cellIdx) => {
            const session = orderedSessions[sessionIdx++];
            if (!session) return null;
            const isFocused = session === focused;
            const isDragging = session === dragSource;
            const isDropTarget = session === dragTarget;
            return (
              <div
                key={session}
                class={[
                  "auto-grid-cell",
                  isFocused && "focused",
                  isDragging && "dragging",
                  isDropTarget && "drop-target",
                ].filter(Boolean).join(" ")}
                onClick={() => handleClick(session)}
                onDblClick={() => handleDoubleClick(session)}
                draggable
                onDragStart={(e: DragEvent) => handleDragStart(session, e)}
                onDragOver={(e: DragEvent) => handleDragOver(session, e)}
                onDragLeave={handleDragLeave}
                onDrop={(e: DragEvent) => handleDrop(session, e)}
                onDragEnd={handleDragEnd}
              >
                <SessionPane session={session} client={client} />
              </div>
            );
          })}
        </div>
      ))}
    </div>
  );
}
