import { useRef, useCallback } from "preact/hooks";
import { tileLayout, focusedSession, viewMode } from "../state/sessions";
import { SessionPane } from "./SessionPane";
import type { WshClient } from "../api/ws";

interface TiledLayoutProps {
  client: WshClient;
}

export function TiledLayout({ client }: TiledLayoutProps) {
  const layout = tileLayout.value;
  const containerRef = useRef<HTMLDivElement>(null);

  if (!layout || layout.sessions.length < 2) {
    viewMode.value = "focused";
    return null;
  }

  const { sessions: tileSessions, sizes } = layout;

  const handleResize = useCallback(
    (index: number, e: PointerEvent) => {
      if (!containerRef.current || !layout) return;
      const rect = containerRef.current.getBoundingClientRect();
      const totalWidth = rect.width;
      const relX = (e.clientX - rect.left) / totalWidth;

      // Calculate cumulative size up to the handle
      let cumBefore = 0;
      for (let i = 0; i < index; i++) cumBefore += sizes[i];
      let cumAfter = cumBefore + sizes[index] + sizes[index + 1];

      const minSize = 0.1;
      const leftSize = Math.max(minSize, Math.min(relX - cumBefore + sizes[index], cumAfter - cumBefore - minSize));
      const rightSize = cumAfter - cumBefore - leftSize;

      const newSizes = [...sizes];
      newSizes[index] = leftSize;
      newSizes[index + 1] = rightSize;
      tileLayout.value = { ...layout, sizes: newSizes };
    },
    [layout, sizes],
  );

  const handleMaximize = (session: string) => {
    focusedSession.value = session;
    tileLayout.value = null;
    viewMode.value = "focused";
  };

  const handleRemovePane = (session: string) => {
    if (!layout) return;
    const idx = tileSessions.indexOf(session);
    if (idx < 0) return;

    const remaining = tileSessions.filter((s) => s !== session);
    if (remaining.length < 2) {
      focusedSession.value = remaining[0] ?? focusedSession.value;
      tileLayout.value = null;
      viewMode.value = "focused";
      return;
    }

    // Redistribute removed pane's size to neighbors
    const removedSize = sizes[idx];
    const newSizes = sizes.filter((_, i) => i !== idx);
    // Give the space to the adjacent pane (prefer left, fallback right)
    const neighbor = idx > 0 ? idx - 1 : 0;
    newSizes[neighbor] += removedSize;

    tileLayout.value = { sessions: remaining, sizes: newSizes };
  };

  return (
    <div class="tiled-layout" ref={containerRef}>
      {tileSessions.map((session, i) => (
        <>
          {i > 0 && (
            <TileResizeHandle
              index={i - 1}
              containerRef={containerRef}
              onDrag={handleResize}
            />
          )}
          <div class="tile-pane" style={{ flex: sizes[i] }}>
            <div class="tile-header">
              <span class="tile-name">{session}</span>
              <div class="tile-actions">
                {tileSessions.length > 2 && (
                  <button
                    class="tile-close"
                    onClick={() => handleRemovePane(session)}
                    title="Remove from tile"
                  >
                    <svg width="8" height="8" viewBox="0 0 8 8">
                      <path
                        d="M1 1L7 7M7 1L1 7"
                        stroke="currentColor"
                        stroke-width="1.2"
                        stroke-linecap="round"
                      />
                    </svg>
                  </button>
                )}
                <button
                  class="tile-maximize"
                  onClick={() => handleMaximize(session)}
                  title="Maximize"
                >
                  <svg width="10" height="10" viewBox="0 0 10 10">
                    <rect
                      x="0.5"
                      y="0.5"
                      width="9"
                      height="9"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="1"
                      rx="1"
                    />
                  </svg>
                </button>
              </div>
            </div>
            <div
              class={`tile-content ${focusedSession.value === session ? "tile-focused" : ""}`}
              onClick={() => {
                focusedSession.value = session;
              }}
            >
              <SessionPane session={session} client={client} />
            </div>
          </div>
        </>
      ))}
    </div>
  );
}

// Per-handle resize component using pointer capture
function TileResizeHandle({
  index,
  containerRef,
  onDrag,
}: {
  index: number;
  containerRef: { current: HTMLDivElement | null };
  onDrag: (index: number, e: PointerEvent) => void;
}) {
  const dragging = useRef(false);

  const handlePointerDown = useCallback(
    (e: PointerEvent) => {
      e.preventDefault();
      dragging.current = true;
      const target = e.currentTarget as HTMLElement;
      target.setPointerCapture(e.pointerId);

      const handlePointerMove = (ev: PointerEvent) => {
        if (!dragging.current) return;
        onDrag(index, ev);
      };

      const handlePointerUp = () => {
        dragging.current = false;
        target.removeEventListener("pointermove", handlePointerMove);
        target.removeEventListener("pointerup", handlePointerUp);
      };

      target.addEventListener("pointermove", handlePointerMove);
      target.addEventListener("pointerup", handlePointerUp);
    },
    [index, onDrag],
  );

  return (
    <div class="tile-resize-handle" onPointerDown={handlePointerDown} />
  );
}
