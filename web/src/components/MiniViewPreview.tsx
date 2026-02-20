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

/** Render a tiny terminal content block showing bottom N lines. */
export function MiniTermContent({ session, maxLines = 6 }: { session: string; maxLines?: number }) {
  const screen = getScreenSignal(session).value;
  // Show the bottom N lines (most recent activity) instead of top
  const start = Math.max(0, screen.lines.length - maxLines);
  const lines = screen.lines.slice(start, start + maxLines);
  return (
    <div class="mini-term-content">
      {lines.map((line: FormattedLine, i: number) => (
        <div key={i} class="mini-term-line">{lineToText(line)}</div>
      ))}
    </div>
  );
}

/** Mini carousel layout: center session with flanking panels. */
function MiniCarousel({ sessions }: { sessions: string[] }) {
  const focused = focusedSession.value;
  const idx = Math.max(0, sessions.indexOf(focused ?? ""));
  const center = sessions[idx];

  if (sessions.length === 1) {
    return (
      <div class="mini-carousel">
        <div class="mini-carousel-center">
          <MiniTermContent session={center} maxLines={4} />
        </div>
      </div>
    );
  }

  const prevIdx = (idx - 1 + sessions.length) % sessions.length;
  const nextIdx = (idx + 1) % sessions.length;

  return (
    <div class="mini-carousel">
      {sessions.length > 2 && (
        <div class="mini-carousel-side mini-carousel-prev">
          <MiniTermContent session={sessions[prevIdx]} maxLines={3} />
        </div>
      )}
      {sessions.length === 2 && (
        <div class="mini-carousel-side mini-carousel-prev">
          <MiniTermContent session={sessions[prevIdx]} maxLines={3} />
        </div>
      )}
      <div class="mini-carousel-center">
        <MiniTermContent session={center} maxLines={4} />
      </div>
      {sessions.length > 2 && (
        <div class="mini-carousel-side mini-carousel-next">
          <MiniTermContent session={sessions[nextIdx]} maxLines={3} />
        </div>
      )}
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
              <MiniTermContent session={s} maxLines={3} />
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
          <MiniTermContent session={currentSession} maxLines={4} />
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
