import { useRef } from "preact/hooks";
import { tileLayout, focusedSession, viewMode } from "../state/sessions";
import { SessionPane } from "./SessionPane";
import { ResizeHandle } from "./ResizeHandle";
import type { WshClient } from "../api/ws";

interface TiledLayoutProps {
  client: WshClient;
}

export function TiledLayout({ client }: TiledLayoutProps) {
  const layout = tileLayout.value;
  const containerRef = useRef<HTMLDivElement>(null);

  if (!layout || layout.sessions.length < 2) {
    // Fall back to focused mode if layout is invalid
    viewMode.value = "focused";
    return null;
  }

  const [left, right] = layout.sessions;
  const [leftSize, rightSize] = layout.sizes;

  const handleResize = (ratio: number) => {
    tileLayout.value = {
      ...layout,
      sizes: [ratio, 1 - ratio],
    };
  };

  const handleMaximize = (session: string) => {
    focusedSession.value = session;
    tileLayout.value = null;
    viewMode.value = "focused";
  };

  return (
    <div class="tiled-layout" ref={containerRef}>
      <div class="tile-pane" style={{ flex: leftSize }}>
        <div class="tile-header">
          <span class="tile-name">{left}</span>
          <button
            class="tile-maximize"
            onClick={() => handleMaximize(left)}
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
        <div
          class={`tile-content ${focusedSession.value === left ? "tile-focused" : ""}`}
          onClick={() => {
            focusedSession.value = left;
          }}
        >
          <SessionPane session={left} client={client} />
        </div>
      </div>

      <ResizeHandle containerRef={containerRef} onResize={handleResize} />

      <div class="tile-pane" style={{ flex: rightSize }}>
        <div class="tile-header">
          <span class="tile-name">{right}</span>
          <button
            class="tile-maximize"
            onClick={() => handleMaximize(right)}
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
        <div
          class={`tile-content ${focusedSession.value === right ? "tile-focused" : ""}`}
          onClick={() => {
            focusedSession.value = right;
          }}
        >
          <SessionPane session={right} client={client} />
        </div>
      </div>
    </div>
  );
}
