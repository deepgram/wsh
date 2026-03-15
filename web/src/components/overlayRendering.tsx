import { h } from "preact";
import type { OverlaySpan, RegionWrite } from "../api/types";
import { overlaySpanStyle } from "../utils/terminal";

export function renderSpans(spans: OverlaySpan[]): h.JSX.Element[] {
  return spans.map((span, i) => (
    <span key={i} style={overlaySpanStyle(span)}>
      {span.text}
    </span>
  ));
}

export function renderRegionWrites(
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
