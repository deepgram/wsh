import { useCallback } from "preact/hooks";
import type { WshClient } from "../api/ws";
import { dragState, dropTargetTag, handleGroupDragOver, handleGroupDragLeave, handleGroupDrop, endDrag } from "../hooks/useDragDrop";
import { groups, selectedGroups, collapsedGroups, toggleGroupCollapsed, getGroupStatusCounts } from "../state/groups";
import { connectionState } from "../state/sessions";
import { ThumbnailCell } from "./ThumbnailCell";
import { ThemePicker } from "./ThemePicker";

interface SidebarProps {
  client: WshClient;
  collapsed: boolean;
  onToggleCollapse: () => void;
}

export function Sidebar({ client, collapsed, onToggleCollapse }: SidebarProps) {
  const allGroups = groups.value;
  const selected = selectedGroups.value;
  const connState = connectionState.value;
  const _dragState = dragState.value;
  const dropTarget = dropTargetTag.value;

  const handleGroupClick = useCallback((tag: string, e: MouseEvent) => {
    if (e.ctrlKey || e.metaKey) {
      const current = selectedGroups.value;
      if (current.includes(tag)) {
        // Prevent deselecting the last group
        const filtered = current.filter((t) => t !== tag);
        if (filtered.length > 0) {
          selectedGroups.value = filtered;
        }
      } else {
        selectedGroups.value = [...current, tag];
      }
    } else {
      selectedGroups.value = [tag];
    }
  }, []);

  const handleNewSession = useCallback(() => {
    client.createSession().catch((e) => {
      console.error("Failed to create session:", e);
    });
  }, [client]);

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
            onClick={(e: MouseEvent) => handleGroupClick(g.tag, e)}
            title={`${g.label} (${g.sessions.length})`}
            role="button"
            aria-label={`Group: ${g.label}, ${g.sessions.length} sessions`}
            aria-selected={selected.includes(g.tag)}
          >
            <span class="sidebar-icon-count">{g.sessions.length}</span>
            {g.badgeCount > 0 && <span class="sidebar-badge" aria-label={`${g.badgeCount} sessions need attention`}>{g.badgeCount}</span>}
          </div>
        ))}
        <div style={{ flex: 1 }} />
        <div class={`status-dot ${connState}`} title={connState} />
        <button class="sidebar-icon sidebar-new-icon" onClick={handleNewSession} title="New session">
          +
        </button>
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
        {allGroups.map((g) => {
          const isCollapsed = collapsedGroups.value.has(g.tag);
          const { running, idle } = getGroupStatusCounts(g);
          const sortedSessions = [...g.sessions].sort();

          return (
            <div
              key={g.tag}
              class={`sidebar-group ${selected.includes(g.tag) ? "selected" : ""} ${dropTarget === g.tag ? "drop-target" : ""}`}
              onClick={(e: MouseEvent) => handleGroupClick(g.tag, e)}
              onDragOver={(e: DragEvent) => handleGroupDragOver(g.tag, e)}
              onDragLeave={handleGroupDragLeave}
              onDrop={(e: DragEvent) => handleGroupDrop(g.tag, e, client)}
              role="button"
              aria-label={`Group: ${g.label}, ${g.sessions.length} sessions`}
              aria-selected={selected.includes(g.tag)}
            >
              <div class="sidebar-group-header">
                <span
                  class={`sidebar-group-chevron ${isCollapsed ? "" : "expanded"}`}
                  onClick={(e: MouseEvent) => { e.stopPropagation(); toggleGroupCollapsed(g.tag); }}
                >
                  &#9656;
                </span>
                <span class="sidebar-group-label">{g.label}</span>
                {isCollapsed && g.sessions.length > 0 && (
                  <div class="sidebar-status-chips">
                    {idle > 0 && <span class="sidebar-status-chip idle">{idle}</span>}
                    {running > 0 && <span class="sidebar-status-chip running">{running}</span>}
                  </div>
                )}
                {!isCollapsed && (
                  <span class="sidebar-group-count">{g.sessions.length}</span>
                )}
                {g.badgeCount > 0 && <span class="sidebar-badge" aria-label={`${g.badgeCount} sessions need attention`}>{g.badgeCount}</span>}
              </div>
              {!isCollapsed && sortedSessions.length > 0 && (
                <div class="thumb-grid">
                  {sortedSessions.map((s) => (
                    <ThumbnailCell key={s} session={s} client={client} />
                  ))}
                </div>
              )}
            </div>
          );
        })}
      </div>
      <div class="sidebar-hints">
        <button
          class="sidebar-hint"
          onClick={() => window.dispatchEvent(new KeyboardEvent("keydown", { key: "K", ctrlKey: true, shiftKey: true, bubbles: true }))}
        >
          <kbd>^⇧K</kbd> palette
        </button>
        <span class="sidebar-hint-sep">&middot;</span>
        <button
          class="sidebar-hint"
          onClick={() => window.dispatchEvent(new KeyboardEvent("keydown", { key: "/", ctrlKey: true, shiftKey: true, bubbles: true }))}
        >
          <kbd>^⇧/</kbd> shortcuts
        </button>
      </div>
      <div class="sidebar-footer">
        <div class={`status-dot ${connState}`} title={connState} />
        <ThemePicker />
        <div style={{ flex: 1 }} />
        <button class="sidebar-new-session-btn" onClick={handleNewSession} title="New session">
          + New
        </button>
      </div>
      <div class="sr-only" aria-live="polite">
        {allGroups.map((g) => {
          const { running, idle } = getGroupStatusCounts(g);
          return `${g.label}: ${running} running, ${idle} idle`;
        }).join(". ")}
      </div>
    </div>
  );
}
