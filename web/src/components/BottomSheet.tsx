import { useState, useCallback } from "preact/hooks";
import type { WshClient } from "../api/ws";
import { groups, selectedGroups } from "../state/groups";
import { connectionState, focusedSession } from "../state/sessions";

interface BottomSheetProps {
  client: WshClient;
}

export function BottomSheet({ client }: BottomSheetProps) {
  const [expanded, setExpanded] = useState(false);
  const allGroups = groups.value;
  const selected = selectedGroups.value;
  const connState = connectionState.value;

  const primaryGroup = allGroups.find(g => g.tag === selected[0]) || allGroups[0];

  const handleGroupClick = useCallback((tag: string) => {
    selectedGroups.value = [tag];
    setExpanded(false);
  }, []);

  const handleNewSession = useCallback(() => {
    client.createSession().then((info) => {
      focusedSession.value = info.name;
    }).catch(() => {});
    setExpanded(false);
  }, [client]);

  return (
    <>
      {/* Tab bar at bottom */}
      <div class="bottom-sheet-tab" onClick={() => setExpanded(!expanded)} role="button" aria-expanded={expanded} aria-label={`Session groups: ${primaryGroup?.label || "Sessions"}`}>
        <div class={`status-dot ${connState}`} />
        <span class="bottom-sheet-group-name">{primaryGroup?.label || "Sessions"}</span>
        <div style={{ flex: 1 }} />
        <button class="bottom-sheet-new-btn" onClick={(e: MouseEvent) => { e.stopPropagation(); handleNewSession(); }} aria-label="New session">
          +
        </button>
        <span class="bottom-sheet-chevron">{expanded ? "\u25BC" : "\u25B2"}</span>
      </div>

      {/* Expanded sheet */}
      {expanded && (
        <>
          <div class="bottom-sheet-backdrop" onClick={() => setExpanded(false)} />
          <div class="bottom-sheet-panel">
            <div class="bottom-sheet-handle" />
            <div class="bottom-sheet-groups">
              {allGroups.map(g => (
                <div
                  key={g.tag}
                  class={`bottom-sheet-group ${selected.includes(g.tag) ? "selected" : ""}`}
                  onClick={() => handleGroupClick(g.tag)}
                  role="button"
                  aria-label={`Group: ${g.label}, ${g.sessions.length} sessions`}
                >
                  <span class="bottom-sheet-group-label">{g.label}</span>
                  <span class="bottom-sheet-group-count">{g.sessions.length}</span>
                </div>
              ))}
            </div>
          </div>
        </>
      )}
    </>
  );
}
