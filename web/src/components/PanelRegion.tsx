import { h } from "preact";
import type { Panel } from "../api/types";
import { overlayColorToCSS } from "../utils/terminal";
import { renderSpans, renderRegionWrites } from "./overlayRendering";

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
