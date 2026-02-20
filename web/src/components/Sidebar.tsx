import type { WshClient } from "../api/ws";
import { groups, selectedGroups } from "../state/groups";
import { connectionState } from "../state/sessions";

interface SidebarProps {
  client: WshClient;
  collapsed: boolean;
  onToggleCollapse: () => void;
}

export function Sidebar({ client, collapsed, onToggleCollapse }: SidebarProps) {
  const allGroups = groups.value;
  const selected = selectedGroups.value;
  const connState = connectionState.value;

  if (collapsed) {
    return (
      <div class="sidebar-collapsed">
        <button class="sidebar-expand-btn" onClick={onToggleCollapse} title="Expand sidebar">
          &#9656;
        </button>
        {allGroups.map((g) => (
          <div
            key={g.tag}
            class={`sidebar-icon ${selected.includes(g.tag) ? "active" : ""}`}
            onClick={() => { selectedGroups.value = [g.tag]; }}
            title={g.label}
          >
            {g.badgeCount > 0 && <span class="sidebar-badge">{g.badgeCount}</span>}
          </div>
        ))}
      </div>
    );
  }

  return (
    <div class="sidebar-content">
      <div class="sidebar-header">
        <span class="sidebar-title">Sessions</span>
        <button class="sidebar-collapse-btn" onClick={onToggleCollapse} title="Collapse sidebar">
          &#9666;
        </button>
      </div>
      <div class="sidebar-groups">
        {allGroups.map((g) => (
          <div
            key={g.tag}
            class={`sidebar-group ${selected.includes(g.tag) ? "selected" : ""}`}
            onClick={() => { selectedGroups.value = [g.tag]; }}
          >
            <div class="sidebar-group-header">
              <span class="sidebar-group-label">{g.label}</span>
              <span class="sidebar-group-count">{g.sessions.length}</span>
              {g.badgeCount > 0 && <span class="sidebar-badge">{g.badgeCount}</span>}
            </div>
          </div>
        ))}
      </div>
      <div class="sidebar-footer">
        <div class={`status-dot ${connState}`} />
        <button class="sidebar-new-session-btn" title="New session">+</button>
      </div>
    </div>
  );
}
