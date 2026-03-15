import { h } from "preact";
import type { Overlay } from "../api/types";
import { overlayColorToCSS } from "../utils/terminal";
import { renderSpans, renderRegionWrites } from "./overlayRendering";

interface OverlayLayerProps {
  overlays: Overlay[];
  charWidth: number;
  charHeight: number;
}

export function OverlayLayer({ overlays, charWidth, charHeight }: OverlayLayerProps) {
  if (overlays.length === 0) return null;

  return (
    <div class="overlay-layer">
      {overlays.map((o) => (
        <div
          key={o.id}
          class="overlay-item"
          style={{
            left: `${o.x * charWidth}px`,
            top: `${o.y * charHeight}px`,
            width: `${o.width * charWidth}px`,
            height: `${o.height * charHeight}px`,
            zIndex: o.z,
            ...(o.background
              ? { backgroundColor: overlayColorToCSS(o.background.bg) }
              : {}),
          }}
        >
          {renderSpans(o.spans)}
          {renderRegionWrites(o.region_writes ?? [], charWidth, charHeight)}
        </div>
      ))}
    </div>
  );
}
