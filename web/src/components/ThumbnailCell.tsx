import { useState, useRef, useEffect, useCallback } from "preact/hooks";
import type { WshClient } from "../api/ws";
import { sessionStatuses, type SessionStatus } from "../state/groups";
import { focusedSession, sessionInfoMap } from "../state/sessions";
import { startSessionDrag, endDrag } from "../hooks/useDragDrop";
import { MiniTermContent } from "./MiniViewPreview";
import { TagEditor } from "./TagEditor";

interface ThumbnailCellProps {
  session: string;
  client: WshClient;
}

function statusLabel(status: SessionStatus | undefined): string {
  return status === "idle" ? "Idle" : "Running";
}

export function ThumbnailCell({ session, client }: ThumbnailCellProps) {
  const status = sessionStatuses.value.get(session);
  const dotClass = status === "idle" ? "status-dot-green" : "status-dot-amber";
  const info = sessionInfoMap.value.get(session);
  const serverName = info?.server;
  const [hovered, setHovered] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [renameValue, setRenameValue] = useState(session);
  const [showTagEditor, setShowTagEditor] = useState(false);
  const renameRef = useRef<HTMLInputElement>(null);

  // Focus rename input when entering rename mode
  useEffect(() => {
    if (renaming) {
      renameRef.current?.focus();
      renameRef.current?.select();
    }
  }, [renaming]);

  const handleRenameSubmit = useCallback(() => {
    const trimmed = renameValue.trim();
    if (trimmed && trimmed !== session) {
      client.updateSession(session, { name: trimmed }).catch((e) => {
        console.error("Failed to rename session:", e);
      });
    }
    setRenaming(false);
  }, [renameValue, session, client]);

  const handleRenameKeyDown = useCallback((e: KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      handleRenameSubmit();
    } else if (e.key === "Escape") {
      e.preventDefault();
      setRenaming(false);
      setRenameValue(session);
    }
  }, [handleRenameSubmit, session]);

  const handleThumbClick = useCallback((e: MouseEvent) => {
    // Don't navigate if clicking on name, tag icon, or rename input
    const target = e.target as HTMLElement;
    if (target.closest(".thumb-name, .thumb-tag-btn, .thumb-rename-input, .tag-editor")) return;
    focusedSession.value = session;
  }, [session]);

  return (
    <div
      class={`thumb-cell ${focusedSession.value === session ? "focused" : ""}`}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => { setHovered(false); if (!showTagEditor) setRenaming(false); }}
      onClick={handleThumbClick}
      draggable
      onDragStart={(e: DragEvent) => startSessionDrag(session, e)}
      onDragEnd={endDrag}
      role="button"
      aria-label={`Session ${session}, ${statusLabel(status)}`}
    >
      {/* Terminal preview */}
      <div class="thumb-preview">
        <MiniTermContent session={session} />
      </div>

      {/* Server badge — top-left, shown for remote sessions */}
      {serverName && (
        <span class="server-badge" title={`Server: ${serverName}`}>{serverName}</span>
      )}

      {/* Status dot — always visible in lower-right */}
      {!hovered && (
        <span class={`thumb-status-dot ${dotClass}`} aria-label={statusLabel(status)} />
      )}

      {/* Hover overlay — bottom bar with name + status dot */}
      {hovered && (
        <div class="thumb-overlay">
          {renaming ? (
            <input
              ref={renameRef}
              type="text"
              class="thumb-rename-input"
              value={renameValue}
              onInput={(e) => setRenameValue((e.target as HTMLInputElement).value)}
              onKeyDown={handleRenameKeyDown}
              onBlur={handleRenameSubmit}
              onClick={(e: MouseEvent) => e.stopPropagation()}
            />
          ) : (
            <span
              class="thumb-name"
              onClick={(e: MouseEvent) => { e.stopPropagation(); setRenaming(true); setRenameValue(session); }}
              title="Click to rename"
            >
              {session}
            </span>
          )}
          <span class={`mini-status-dot ${dotClass}`} />
        </div>
      )}

      {/* Tag icon — upper-right, visible on hover */}
      {hovered && (
        <button
          class="thumb-tag-btn"
          onClick={(e: MouseEvent) => { e.stopPropagation(); setShowTagEditor(!showTagEditor); }}
          title="Edit tags"
        >
          &#9868;
        </button>
      )}

      {/* Tag editor popover */}
      {showTagEditor && (
        <div class="thumb-tag-popover">
          <TagEditor
            session={session}
            client={client}
            onClose={() => setShowTagEditor(false)}
          />
        </div>
      )}
    </div>
  );
}
