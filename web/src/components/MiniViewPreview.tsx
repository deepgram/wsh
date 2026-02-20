import { useRef, useEffect, useState } from "preact/hooks";
import { getScreenSignal } from "../state/terminal";
import { getViewModeForGroup, quiescenceQueues } from "../state/groups";
import { focusedSession } from "../state/sessions";
import type { FormattedLine } from "../api/types";
import type { Group } from "../state/groups";

interface MiniViewPreviewProps {
  group: Group;
}

/** Extract plain text from a FormattedLine. */
function lineToText(line: FormattedLine): string {
  if (typeof line === "string") return line;
  return line.map((s) => s.text).join("");
}

/**
 * Render a scaled-down replica of the full terminal screen.
 * Renders all visible lines at a base font size, then uses CSS
 * transform to scale down to fit the container.
 */
export function MiniTermContent({ session }: { session: string }) {
  const screen = getScreenSignal(session).value;
  const containerRef = useRef<HTMLDivElement>(null);
  const innerRef = useRef<HTMLDivElement>(null);
  const [scale, setScale] = useState(1);

  useEffect(() => {
    const container = containerRef.current;
    const inner = innerRef.current;
    if (!container || !inner) return;

    const ro = new ResizeObserver(() => {
      const cw = container.clientWidth;
      const ch = container.clientHeight;
      const iw = inner.scrollWidth;
      const ih = inner.scrollHeight;
      if (iw > 0 && ih > 0) {
        setScale(Math.min(cw / iw, ch / ih, 1));
      }
    });
    ro.observe(container);
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

/** Mini carousel layout: film-strip row of thumbnails with active highlighted. */
function MiniCarousel({ sessions }: { sessions: string[] }) {
  const focused = focusedSession.value;
  const idx = Math.max(0, sessions.indexOf(focused ?? ""));

  return (
    <div class="mini-carousel">
      {sessions.map((s, i) => (
        <div key={s} class={`mini-carousel-thumb ${i === idx ? "active" : ""}`}>
          <MiniTermContent session={s} />
        </div>
      ))}
    </div>
  );
}

/** Mini grid layout matching NxM from the full AutoGrid. */
function MiniGrid({ sessions }: { sessions: string[] }) {
  const cols = Math.ceil(Math.sqrt(sessions.length));
  const rows: string[][] = [];
  for (let i = 0; i < sessions.length; i += cols) {
    rows.push(sessions.slice(i, i + cols));
  }

  return (
    <div class="mini-grid">
      {rows.map((row, ri) => (
        <div key={ri} class="mini-grid-row">
          {row.map((s) => (
            <div key={s} class="mini-grid-cell">
              <MiniTermContent session={s} />
            </div>
          ))}
        </div>
      ))}
    </div>
  );
}

/** Mini queue layout: pending count, highlighted current, muted active. */
function MiniQueue({ sessions, groupTag }: { sessions: string[]; groupTag: string }) {
  const queue = quiescenceQueues.value[groupTag] || [];
  const pending = queue.filter((e) => e.status === "pending");
  const currentSession = pending[0]?.session || sessions[0];

  return (
    <div class="mini-queue">
      <div class="mini-queue-bar">
        <span class="mini-queue-count">{pending.length} pending</span>
      </div>
      {currentSession && (
        <div class="mini-queue-current">
          <MiniTermContent session={currentSession} />
        </div>
      )}
      {sessions.length > 1 && (
        <div class="mini-queue-others">
          {sessions.filter((s) => s !== currentSession).slice(0, 3).map((s) => (
            <div key={s} class="mini-queue-thumb" />
          ))}
        </div>
      )}
    </div>
  );
}

/** Renders a miniature replica of the group's active view mode. */
export function MiniViewPreview({ group }: MiniViewPreviewProps) {
  const mode = getViewModeForGroup(group.tag);

  if (group.sessions.length === 0) {
    return <div class="mini-view-empty">No sessions</div>;
  }

  switch (mode) {
    case "carousel":
      return <MiniCarousel sessions={group.sessions} />;
    case "tiled":
      return <MiniGrid sessions={group.sessions} />;
    case "queue":
      return <MiniQueue sessions={group.sessions} groupTag={group.tag} />;
    default:
      return <MiniCarousel sessions={group.sessions} />;
  }
}
