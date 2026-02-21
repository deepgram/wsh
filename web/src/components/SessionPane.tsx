import { useState, useEffect, useRef, useCallback } from "preact/hooks";
import { Terminal } from "./Terminal";
import { InputBar } from "./InputBar";
import { TagEditor } from "./TagEditor";
import type { WshClient } from "../api/ws";
import { sessionStatuses, type SessionStatus } from "../state/groups";
import { sessionInfoMap } from "../state/sessions";

interface SessionPaneProps {
  session: string;
  client: WshClient;
}

function statusLabel(status: SessionStatus | undefined): string {
  return status === "idle" ? "Idle" : "Running";
}

export function SessionPane({ session, client }: SessionPaneProps) {
  const [isMobile, setIsMobile] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [renameValue, setRenameValue] = useState(session);
  const [showTagEditor, setShowTagEditor] = useState(false);
  const renameRef = useRef<HTMLInputElement>(null);

  const status = sessionStatuses.value.get(session);
  const dotClass = status === "idle" ? "status-dot-green" : "status-dot-amber";
  const info = sessionInfoMap.value.get(session);
  const tags = info?.tags ?? [];

  useEffect(() => {
    const mq = window.matchMedia("(pointer: coarse)");
    setIsMobile(mq.matches);
    const handler = (e: MediaQueryListEvent) => setIsMobile(e.matches);
    mq.addEventListener("change", handler);
    return () => mq.removeEventListener("change", handler);
  }, []);

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

  const removeTag = useCallback((tag: string) => {
    client.updateSession(session, { remove_tags: [tag] }).catch((e) => {
      console.error("Failed to remove tag:", e);
    });
  }, [client, session]);

  return (
    <div class="session-pane">
      <div class="pane-title-bar">
        <div class="pane-title-left">
          <span class={`mini-status-dot ${dotClass}`} aria-label={statusLabel(status)} />
          {renaming ? (
            <input
              ref={renameRef}
              type="text"
              class="pane-rename-input"
              value={renameValue}
              onInput={(e) => setRenameValue((e.target as HTMLInputElement).value)}
              onKeyDown={handleRenameKeyDown}
              onBlur={handleRenameSubmit}
            />
          ) : (
            <span
              class="pane-title-name"
              onClick={() => { setRenaming(true); setRenameValue(session); }}
              title="Click to rename"
            >
              {session}
            </span>
          )}
        </div>
        <div class="pane-title-right">
          {tags.map((tag) => (
            <span key={tag} class="pane-tag-pill">
              {tag}
              <button class="pane-tag-remove" onClick={() => removeTag(tag)}>×</button>
            </span>
          ))}
          <div class="pane-tag-add-wrap">
            <button
              class="pane-tag-add-btn"
              onClick={() => setShowTagEditor(!showTagEditor)}
              title="Edit tags"
            >
              +
            </button>
            {showTagEditor && (
              <div class="pane-tag-popover">
                <TagEditor
                  session={session}
                  client={client}
                  onClose={() => setShowTagEditor(false)}
                />
              </div>
            )}
          </div>
        </div>
      </div>
      <Terminal session={session} client={client} captureInput={!isMobile} />
      {isMobile && <InputBar session={session} client={client} />}
    </div>
  );
}
