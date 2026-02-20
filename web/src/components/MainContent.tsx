import type { WshClient } from "../api/ws";
import { selectedGroups, getViewModeForGroup, activeGroupSessions } from "../state/groups";
import { focusedSession } from "../state/sessions";
import { AutoGrid } from "./AutoGrid";
import { DepthCarousel } from "./DepthCarousel";
import { QueueView } from "./QueueView";
import { SessionPane } from "./SessionPane";

interface MainContentProps {
  client: WshClient;
}

export function MainContent({ client }: MainContentProps) {
  const selected = selectedGroups.value;
  const primaryTag = selected[0] || "all";
  const mode = getViewModeForGroup(primaryTag);
  const sessions = activeGroupSessions.value;
  const focused = focusedSession.value;

  const groupLabel = primaryTag === "all" ? "All Sessions" : primaryTag;

  if (sessions.length === 0) {
    return (
      <div class="main-content">
        <div class="main-header">
          <span class="main-group-name">{groupLabel}</span>
        </div>
        <div class="main-body main-empty">
          No sessions
        </div>
      </div>
    );
  }

  // Default/carousel mode
  if (mode === "carousel") {
    return (
      <div class="main-content">
        <div class="main-header">
          <span class="main-group-name">{groupLabel}</span>
          <span class="main-session-count">{sessions.length} sessions</span>
        </div>
        <div class="main-body">
          <DepthCarousel sessions={sessions} client={client} />
        </div>
      </div>
    );
  }

  if (mode === "tiled") {
    return (
      <div class="main-content">
        <div class="main-header">
          <span class="main-group-name">{groupLabel}</span>
          <span class="main-session-count">{sessions.length} sessions</span>
        </div>
        <div class="main-body">
          <AutoGrid sessions={sessions} client={client} />
        </div>
      </div>
    );
  }

  // Queue mode
  if (mode === "queue") {
    return (
      <div class="main-content">
        <div class="main-header">
          <span class="main-group-name">{groupLabel}</span>
          <span class="main-session-count">{sessions.length} sessions</span>
        </div>
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
      <div class="main-header">
        <span class="main-group-name">{groupLabel}</span>
        <span class="main-session-count">{sessions.length} sessions</span>
      </div>
      <div class="main-body">
        {displaySession && <SessionPane session={displaySession} client={client} />}
      </div>
    </div>
  );
}
