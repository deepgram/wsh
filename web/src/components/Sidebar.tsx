import { useCallback, useState } from "preact/hooks";
import type { WshClient } from "../api/ws";
import { dragState, dropTargetTag, startSessionDrag, handleGroupDragOver, handleGroupDragLeave, handleGroupDrop, endDrag } from "../hooks/useDragDrop";
import { groups, selectedGroups, sessionStatuses, type SessionStatus } from "../state/groups";
import { connectionState } from "../state/sessions";
import { MiniViewPreview } from "./MiniViewPreview";
import { TagEditor } from "./TagEditor";
import { ThemePicker } from "./ThemePicker";

interface SidebarProps {
  client: WshClient;
  collapsed: boolean;
  onToggleCollapse: () => void;
}

function statusLabel(status: SessionStatus | undefined): string {
  return status === "quiescent" ? "Idle"
    : status === "exited" ? "Exited"
    : "Running";
}

function StatusDot({ status }: { status: SessionStatus | undefined }) {
  const cls = status === "quiescent" ? "status-dot-amber"
    : status === "exited" ? "status-dot-grey"
    : "status-dot-green";
  return <span class={`mini-status-dot ${cls}`} aria-label={statusLabel(status)} />;
}

export function Sidebar({ client, collapsed, onToggleCollapse }: SidebarProps) {
  const allGroups = groups.value;
  const selected = selectedGroups.value;
  const connState = connectionState.value;
  const statuses = sessionStatuses.value;
  const _dragState = dragState.value;
  const dropTarget = dropTargetTag.value;
  const [editingSession, setEditingSession] = useState<string | null>(null);

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
        {allGroups.map((g) => (
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
              <span class="sidebar-group-label">{g.label}</span>
              <span class="sidebar-group-count">{g.sessions.length}</span>
              {g.badgeCount > 0 && <span class="sidebar-badge" aria-label={`${g.badgeCount} sessions need attention`}>{g.badgeCount}</span>}
            </div>
            {/* Mini view mode preview */}
            {g.sessions.length > 0 && (
              <div class="sidebar-preview-area">
                <MiniViewPreview group={g} />
              </div>
            )}
            {/* Session list for drag-to-tag and context menu */}
            {g.sessions.length > 0 && (
              <div class="sidebar-session-list">
                {g.sessions.map((s) => (
                  <div
                    key={s}
                    class="sidebar-session-item"
                    draggable
                    onDragStart={(e: DragEvent) => startSessionDrag(s, e)}
                    onDragEnd={endDrag}
                    onContextMenu={(e: MouseEvent) => { e.preventDefault(); e.stopPropagation(); setEditingSession(s); }}
                  >
                    <StatusDot status={statuses.get(s)} />
                    <span class="sidebar-session-name">{s}</span>
                    <button
                      class="tag-edit-btn"
                      onClick={(e: MouseEvent) => { e.stopPropagation(); setEditingSession(s); }}
                      title="Edit tags"
                    >
                      +
                    </button>
                    {editingSession === s && (
                      <TagEditor
                        session={s}
                        client={client}
                        onClose={() => setEditingSession(null)}
                      />
                    )}
                  </div>
                ))}
              </div>
            )}
            {/* Timestamp */}
            {g.sessions.length > 0 && (
              <div class="sidebar-group-timestamp">
                {(() => {
                  // Show "Last active" based on group activity
                  const hasQuiescent = g.sessions.some((s) => statuses.get(s) === "quiescent");
                  const allExited = g.sessions.every((s) => statuses.get(s) === "exited");
                  if (allExited) return "Exited";
                  if (hasQuiescent) return "Idle";
                  return "Active";
                })()}
              </div>
            )}
          </div>
        ))}
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
        {allGroups.map((g) =>
          g.sessions.map((s) => {
            const st = statuses.get(s);
            return `${s}: ${statusLabel(st)}`;
          })
        ).flat().join(". ")}
      </div>
    </div>
  );
}
