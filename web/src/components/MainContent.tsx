import { useEffect } from "preact/hooks";
import type { WshClient } from "../api/ws";
import { selectedGroups, getViewModeForGroup, setViewModeForGroup, activeGroupSessions } from "../state/groups";
import { focusedSession, type ViewMode } from "../state/sessions";
import { AutoGrid } from "./AutoGrid";
import { DepthCarousel } from "./DepthCarousel";
import { QueueView } from "./QueueView";
import { SessionPane } from "./SessionPane";

interface MainContentProps {
  client: WshClient;
}

function ViewModeToggle({ mode, groupTag }: { mode: ViewMode; groupTag: string }) {
  return (
    <div class="view-mode-toggle" role="radiogroup" aria-label="View mode">
      <button
        class={`view-mode-btn ${mode === "carousel" ? "active" : ""}`}
        onClick={() => setViewModeForGroup(groupTag, "carousel")}
        title="Carousel (Ctrl+Shift+F)"
        role="radio"
        aria-checked={mode === "carousel"}
      >
        &#9655;
      </button>
      <button
        class={`view-mode-btn ${mode === "tiled" ? "active" : ""}`}
        onClick={() => setViewModeForGroup(groupTag, "tiled")}
        title="Tiled (Ctrl+Shift+G)"
        role="radio"
        aria-checked={mode === "tiled"}
      >
        &#9638;
      </button>
      <button
        class={`view-mode-btn ${mode === "queue" ? "active" : ""}`}
        onClick={() => setViewModeForGroup(groupTag, "queue")}
        title="Queue (Ctrl+Shift+Q)"
        role="radio"
        aria-checked={mode === "queue"}
      >
        &#9776;
      </button>
    </div>
  );
}

export function MainContent({ client }: MainContentProps) {
  const selected = selectedGroups.value;
  const primaryTag = selected[0] || "all";
  const mode = getViewModeForGroup(primaryTag);
  const sessions = activeGroupSessions.value;
  const focused = focusedSession.value;
  const isMobile = window.innerWidth < 640;

  // Mobile: single focused session, no view mode chrome
  if (isMobile) {
    const displaySession = focused && sessions.includes(focused) ? focused : sessions[0];
    return (
      <div class="main-content">
        <div class="main-body">
          {displaySession ? (
            <SessionPane key={displaySession} session={displaySession} client={client} />
          ) : (
            <div class="main-empty">No sessions</div>
          )}
        </div>
      </div>
    );
  }

  const groupLabel = primaryTag === "all" ? "All Sessions" : primaryTag;

  // Keyboard shortcuts for view mode switching
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (!e.ctrlKey || !e.shiftKey) return;
      if (e.altKey || e.metaKey) return;

      const tag = selectedGroups.value[0] || "all";
      if (e.key === "f" || e.key === "F") {
        e.preventDefault();
        setViewModeForGroup(tag, "carousel");
      } else if (e.key === "g" || e.key === "G") {
        e.preventDefault();
        setViewModeForGroup(tag, "tiled");
      } else if (e.key === "q" || e.key === "Q") {
        e.preventDefault();
        setViewModeForGroup(tag, "queue");
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  if (sessions.length === 0) {
    return (
      <div class="main-content">
        <div class="main-header">
          <span class="main-group-name">{groupLabel}</span>
          <div style={{ flex: 1 }} />
          <ViewModeToggle mode={mode} groupTag={primaryTag} />
        </div>
        <div class="main-body main-empty">
          No sessions
        </div>
      </div>
    );
  }

  const header = (
    <div class="main-header">
      <span class="main-group-name">{groupLabel}</span>
      <span class="main-session-count">{sessions.length} sessions</span>
      <div style={{ flex: 1 }} />
      <ViewModeToggle mode={mode} groupTag={primaryTag} />
    </div>
  );

  if (mode === "carousel") {
    return (
      <div class="main-content">
        {header}
        <div class="main-body">
          <DepthCarousel sessions={sessions} client={client} />
        </div>
      </div>
    );
  }

  if (mode === "tiled") {
    return (
      <div class="main-content">
        {header}
        <div class="main-body">
          <AutoGrid sessions={sessions} client={client} />
        </div>
      </div>
    );
  }

  if (mode === "queue") {
    return (
      <div class="main-content">
        {header}
        <div class="main-body">
          <QueueView sessions={sessions} groupTag={primaryTag} client={client} />
        </div>
      </div>
    );
  }

  // Fallback
  const displaySession = focused && sessions.includes(focused) ? focused : sessions[0];
  return (
    <div class="main-content">
      {header}
      <div class="main-body">
        {displaySession && <SessionPane key={displaySession} session={displaySession} client={client} />}
      </div>
    </div>
  );
}
