import { useRef, useEffect, useState } from "preact/hooks";
import { getScreenSignal } from "../state/terminal";
import type { FormattedLine } from "../api/types";

/** Extract plain text from a FormattedLine. */
function lineToText(line: FormattedLine): string {
  if (typeof line === "string") return line;
  return line.map((s) => s.text).join("");
}

/**
 * Render a scaled-down replica of the full terminal screen.
 *
 * The inner div is positioned absolutely and rendered at full font size
 * with no constraints, so it expands to its natural width/height. A
 * ResizeObserver then measures the container and inner sizes, computing
 * a scale factor to shrink the entire terminal to fit the container.
 * This produces a true "thumbnail" showing all rows and columns.
 */
export function MiniTermContent({ session }: { session: string }) {
  const screen = getScreenSignal(session).value;
  const containerRef = useRef<HTMLDivElement>(null);
  const innerRef = useRef<HTMLDivElement>(null);
  const [scale, setScale] = useState(0.1);

  useEffect(() => {
    const container = containerRef.current;
    const inner = innerRef.current;
    if (!container || !inner) return;

    const update = () => {
      const cw = container.clientWidth;
      const ch = container.clientHeight;
      // scrollWidth/Height on an absolutely-positioned element reflects
      // the true unconstrained content size.
      const iw = inner.scrollWidth;
      const ih = inner.scrollHeight;
      if (iw > 0 && ih > 0 && cw > 0 && ch > 0) {
        setScale(Math.min(cw / iw, ch / ih, 1));
      }
    };

    const ro = new ResizeObserver(update);
    ro.observe(container);
    // Also observe inner in case content changes without container resize
    ro.observe(inner);
    return () => ro.disconnect();
  }, [screen.lines.length]);

  return (
    <div class="mini-term-content" ref={containerRef}>
      <div
        class="mini-term-inner"
        ref={innerRef}
        style={{ transform: `scale(${scale})`, transformOrigin: "top left" }}
      >
        {screen.lines.map((line: FormattedLine, i: number) => (
          <div key={i} class="mini-term-line">{lineToText(line)}</div>
        ))}
      </div>
    </div>
  );
}
