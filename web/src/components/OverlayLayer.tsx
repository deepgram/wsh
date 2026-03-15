import { h } from "preact";
import type { Overlay, OverlaySpan, RegionWrite } from "../api/types";
import { overlaySpanStyle, overlayColorToCSS } from "../utils/terminal";

interface OverlayLayerProps {
  overlays: Overlay[];
  charWidth: number;
  charHeight: number;
}

function renderSpans(spans: OverlaySpan[]): h.JSX.Element[] {
  return spans.map((span, i) => (
    <span key={i} style={overlaySpanStyle(span)}>
      {span.text}
    </span>
  ));
}

function renderRegionWrites(
  writes: RegionWrite[],
  charWidth: number,
  charHeight: number,
): h.JSX.Element[] {
  return writes.map((rw, i) => (
    <span
      key={`rw-${i}`}
      class="region-write"
      style={{
        left: `${rw.col * charWidth}px`,
        top: `${rw.row * charHeight}px`,
        ...overlaySpanStyle(rw),
      }}
    >
      {rw.text}
    </span>
  ));
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
