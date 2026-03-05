import { useState, useRef, useEffect, useCallback, useLayoutEffect } from "preact/hooks";
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
  const tags = info?.tags ?? [];
  const [hovered, setHovered] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [renameValue, setRenameValue] = useState(session);
  const [showTagEditor, setShowTagEditor] = useState(false);
  const [popoverPos, setPopoverPos] = useState<{ top: number; right: number } | null>(null);
  const renameRef = useRef<HTMLInputElement>(null);
  const tagBtnRef = useRef<HTMLButtonElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);

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
      client.renameSession(session, trimmed).catch((e) => {
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

  // Clamp popover to viewport after render (before paint)
  useLayoutEffect(() => {
    const el = popoverRef.current;
    if (!showTagEditor || !el || !popoverPos) return;

    const rect = el.getBoundingClientRect();
    const pad = 8;
    const vh = window.innerHeight;
    const vw = window.innerWidth;

    // If popover extends below viewport, move it up
    if (rect.bottom > vh - pad) {
      el.style.top = `${Math.max(pad, vh - rect.height - pad)}px`;
    }

    // If popover extends past left edge (right value too large), pull it back
    if (rect.left < pad) {
      el.style.right = `${Math.max(pad, vw - rect.width - pad)}px`;
    }
  }, [showTagEditor, popoverPos]);

  // Close popover when sidebar scrolls or window resizes
  useEffect(() => {
    if (!showTagEditor) return;
    const close = () => { setShowTagEditor(false); setPopoverPos(null); };
    // Capture phase catches scroll on any ancestor (e.g. sidebar-groups)
    document.addEventListener("scroll", close, true);
    window.addEventListener("resize", close);
    return () => {
      document.removeEventListener("scroll", close, true);
      window.removeEventListener("resize", close);
    };
  }, [showTagEditor]);

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

      {/* Tag count badge — upper-right, visible when has tags and not hovered */}
      {tags.length > 0 && !hovered && (
        <span class="thumb-tag-count">{tags.length}</span>
      )}

      {/* Tag icon — upper-right, visible on hover */}
      {hovered && (
        <button
          ref={tagBtnRef}
          class="thumb-tag-btn"
          onMouseDown={(e: MouseEvent) => e.stopPropagation()}
          onClick={(e: MouseEvent) => {
            e.stopPropagation();
            if (showTagEditor) {
              setShowTagEditor(false);
              setPopoverPos(null);
            } else {
              const rect = tagBtnRef.current?.getBoundingClientRect();
              if (rect) {
                setPopoverPos({
                  top: rect.bottom + 4,
                  right: window.innerWidth - rect.right,
                });
              }
              setShowTagEditor(true);
            }
          }}
          title="Edit tags"
        >
          &#9868;
        </button>
      )}

      {/* Tag editor popover — fixed position to escape overflow:hidden */}
      {showTagEditor && popoverPos && (
        <div
          ref={popoverRef}
          class="thumb-tag-popover"
          style={{
            position: "fixed",
            top: `${popoverPos.top}px`,
            right: `${popoverPos.right}px`,
          }}
          onClick={(e: MouseEvent) => e.stopPropagation()}
        >
          <TagEditor
            session={session}
            client={client}
            onClose={() => { setShowTagEditor(false); setPopoverPos(null); }}
          />
        </div>
      )}
    </div>
  );
}
