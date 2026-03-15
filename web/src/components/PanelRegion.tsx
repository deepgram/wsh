import { h } from "preact";
import type { Panel, OverlaySpan, RegionWrite } from "../api/types";
import { overlaySpanStyle, overlayColorToCSS } from "../utils/terminal";

interface PanelLayout {
  topPanels: Panel[];
  bottomPanels: Panel[];
  hiddenPanelIds: string[];
  terminalRows: number;
}

/**
 * Compute panel layout matching the server's compute_layout() algorithm.
 * Caller must pre-filter by visible and screen_mode before calling.
 */
export function computePanelLayout(
  panels: Panel[],
  totalRows: number,
): PanelLayout {
  // Merge all panels and sort by z descending (highest priority first)
  const sorted = [...panels].sort((a, b) => b.z - a.z);

  let remaining = totalRows;
  const topPanels: Panel[] = [];
  const bottomPanels: Panel[] = [];
  const hiddenPanelIds: string[] = [];

  for (const panel of sorted) {
    if (remaining === 0 || panel.height > remaining) {
      hiddenPanelIds.push(panel.id);
      continue;
    }
    remaining -= panel.height;
    if (panel.position === "top") {
      topPanels.push(panel);
    } else {
      bottomPanels.push(panel);
    }
  }

  // Re-sort within position groups: highest z first (edge toward content)
  topPanels.sort((a, b) => b.z - a.z);
  bottomPanels.sort((a, b) => b.z - a.z);

  return {
    topPanels,
    bottomPanels,
    hiddenPanelIds,
    terminalRows: remaining,
  };
}

interface PanelRegionProps {
  panels: Panel[];
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

export function PanelRegion({ panels, charWidth, charHeight }: PanelRegionProps) {
  return (
    <>
      {panels.map((panel) => (
        <div
          key={panel.id}
          class="panel-region"
          style={{
            height: `${panel.height * charHeight}px`,
            ...(panel.background
              ? { backgroundColor: overlayColorToCSS(panel.background.bg) }
              : {}),
          }}
        >
          {renderSpans(panel.spans)}
          {renderRegionWrites(panel.region_writes ?? [], charWidth, charHeight)}
        </div>
      ))}
    </>
  );
}
