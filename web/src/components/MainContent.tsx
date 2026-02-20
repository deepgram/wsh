import type { WshClient } from "../api/ws";
import { selectedGroups, getViewModeForGroup, activeGroupSessions } from "../state/groups";
import { focusedSession } from "../state/sessions";
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

  // For now, show the focused session as a single SessionPane
  // Later tasks will add carousel, tiled, and queue views
  const displaySession = focused && sessions.includes(focused) ? focused : sessions[0];

  if (!displaySession) {
    return (
      <div class="main-content">
        <div class="main-header">
          <span class="main-group-name">{primaryTag === "all" ? "All Sessions" : primaryTag}</span>
        </div>
        <div class="main-body main-empty">
          No sessions
        </div>
      </div>
    );
  }

  return (
    <div class="main-content">
      <div class="main-header">
        <span class="main-group-name">{primaryTag === "all" ? "All Sessions" : primaryTag}</span>
        <span class="main-session-count">{sessions.length} sessions</span>
      </div>
      <div class="main-body">
        <SessionPane session={displaySession} client={client} />
      </div>
    </div>
  );
}
