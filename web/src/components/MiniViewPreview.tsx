import { useRef, useEffect, useState } from "preact/hooks";
import { getScreenSignal } from "../state/terminal";
import { spanStyle } from "../utils/terminal";
import type { FormattedLine } from "../api/types";

/** Base font size used for rendering the inner terminal content. */
const BASE_FONT_PX = 12;
const LINE_HEIGHT = 1.2;

/**
 * Render a true scaled-down thumbnail of the full terminal screen.
 *
 * The inner div renders all lines at BASE_FONT_PX with styled spans
 * (preserving colors and attributes). Its width is fixed to the
 * terminal's column count in `ch` units, so it matches the real
 * terminal's layout exactly. A ResizeObserver on the container
 * computes a scale factor to shrink the entire terminal to fit.
 */
export function MiniTermContent({ session }: { session: string }) {
  const screen = getScreenSignal(session).value;
  const containerRef = useRef<HTMLDivElement>(null);
  const [scale, setScale] = useState(0.1);

  const { cols, rows, lines } = screen;
  // Natural size of the terminal content at BASE_FONT_PX.
  // We'll measure charWidth from the DOM for accuracy, but also
  // set width in ch units on the inner div so layout is correct.
  const lineHeightPx = BASE_FONT_PX * LINE_HEIGHT;

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const update = () => {
      const cw = container.clientWidth;
      const ch = container.clientHeight;
      if (cw <= 0 || ch <= 0 || cols <= 0 || rows <= 0) return;

      // Measure actual character width by using a hidden measurement span
      const measurer = document.createElement("span");
      measurer.style.fontFamily = "var(--font-mono, 'JetBrains Mono', 'Fira Code', monospace)";
      measurer.style.fontSize = `${BASE_FONT_PX}px`;
      measurer.style.position = "absolute";
      measurer.style.visibility = "hidden";
      measurer.style.whiteSpace = "pre";
      measurer.textContent = "M".repeat(cols);
      document.body.appendChild(measurer);
      const naturalWidth = measurer.offsetWidth;
      document.body.removeChild(measurer);

      const naturalHeight = rows * lineHeightPx;
      setScale(Math.min(cw / naturalWidth, ch / naturalHeight, 1));
    };

    const ro = new ResizeObserver(update);
    ro.observe(container);
    // Also run once immediately after mount
    requestAnimationFrame(update);
    return () => ro.disconnect();
  }, [cols, rows, lineHeightPx]);

  return (
    <div class="mini-term-content" ref={containerRef}>
      <div
        class="mini-term-inner"
        style={{
          transform: `scale(${scale})`,
          transformOrigin: "top left",
          width: `${cols}ch`,
        }}
      >
        {lines.map((line: FormattedLine, i: number) => (
          <div key={i} class="mini-term-line">
            {renderMiniLine(line)}
          </div>
        ))}
      </div>
    </div>
  );
}

/** Render a FormattedLine with styled spans (colors, bold, etc.). */
function renderMiniLine(line: FormattedLine): preact.JSX.Element | string {
  if (typeof line === "string") {
    return line || "\u00A0";
  }
  if (line.length === 0) {
    return "\u00A0";
  }
  return (
    <>
      {line.map((span, i) => (
        <span key={i} style={spanStyle(span)}>
          {span.text}
        </span>
      ))}
    </>
  );
}
