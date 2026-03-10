import { useState, useCallback, useEffect, useRef } from "preact/hooks";
import type { WshClient } from "../api/ws";
import { groups, selectedGroups } from "../state/groups";
import { focusedSession, connectionState, sessionInfoMap } from "../state/sessions";
import { ThemePicker } from "./ThemePicker";

interface MobileDrawerProps {
  client: WshClient;
  open: boolean;
  onClose: () => void;
}

export function MobileDrawer({ client, open, onClose }: MobileDrawerProps) {
  const [autoRotate, setAutoRotate] = useState(true);
  const [menuSession, setMenuSession] = useState<string | null>(null);
  const [visible, setVisible] = useState(false);
  const [closing, setClosing] = useState(false);
  const drawerRef = useRef<HTMLElement>(null);

  useEffect(() => {
    if (open) {
      setVisible(true);
      setClosing(false);
    } else if (visible) {
      setClosing(true);
      const el = drawerRef.current;
      if (el) {
        const onEnd = () => { setVisible(false); setClosing(false); };
        el.addEventListener("animationend", onEnd, { once: true });
        // Fallback in case animationend doesn't fire
        const timer = setTimeout(onEnd, 250);
        return () => { clearTimeout(timer); el.removeEventListener("animationend", onEnd); };
      } else {
        setVisible(false);
        setClosing(false);
      }
    }
  }, [open]);

  if (!visible) return null;

  const allGroups = groups.value;
  const focused = focusedSession.value;
  const connState = connectionState.value;
  const infoMap = sessionInfoMap.value;

  const handleSessionTap = (session: string, groupTag: string) => {
    focusedSession.value = session;
    selectedGroups.value = [groupTag];
    onClose();
  };

  const handleNewSession = () => {
    client.createSession().then((info) => {
      focusedSession.value = info.name;
      selectedGroups.value = ["all"];
    }).catch((e) => console.error("Failed to create session:", e));
    onClose();
  };

  const handleRename = (session: string) => {
    setMenuSession(null);
    const newName = prompt("Rename session:", session);
    if (newName && newName !== session) {
      client.renameSession(session, newName)
        .catch((e: unknown) => console.error("Failed to rename session:", e));
    }
  };

  const handleTag = (session: string) => {
    setMenuSession(null);
    const info = infoMap.get(session);
    const currentTags = info?.tags?.join(", ") || "";
    const input = prompt("Tags (comma-separated):", currentTags);
    if (input === null) return;
    const newTags = input.split(",").map(t => t.trim()).filter(Boolean);
    const oldTags = info?.tags || [];
    const toAdd = newTags.filter(t => !oldTags.includes(t));
    const toRemove = oldTags.filter(t => !newTags.includes(t));
    if (toAdd.length > 0 || toRemove.length > 0) {
      client.updateTags(session, toAdd, toRemove)
        .catch((e: unknown) => console.error("Failed to update tags:", e));
    }
  };

  const handleKill = (session: string) => {
    setMenuSession(null);
    client.killSession(session).then(() => {
      if (focusedSession.value === session) {
        const currentAll = groups.value.find(g => g.tag === "all")?.sessions || [];
        const remaining = currentAll.filter(s => s !== session);
        focusedSession.value = remaining[0] || null;
      }
    }).catch((e) => console.error("Failed to kill session:", e));
  };

  const toggleAutoRotate = useCallback(() => {
    const next = !autoRotate;
    setAutoRotate(next);
    try {
      const orient = screen.orientation as any;
      if (next) {
        orient.unlock?.();
      } else {
        orient.lock?.("portrait")?.catch?.(() => {});
      }
    } catch {
      // orientation lock not supported
    }
  }, [autoRotate]);

  const displayGroups = allGroups.filter(g => g.tag !== "all" && g.sessions.length > 0);
  const allSessions = allGroups.find(g => g.tag === "all")?.sessions || [];
  const showFlat = displayGroups.length === 0;

  const renderSession = (session: string, groupTag: string) => (
    <div key={session} class={`mobile-drawer-session ${session === focused ? "active" : ""}`}>
      <span
        class="mobile-drawer-session-name"
        onClick={() => handleSessionTap(session, groupTag)}
      >
        {session}
      </span>
      <span
        class="mobile-drawer-session-menu-btn"
        onClick={(e) => { e.stopPropagation(); setMenuSession(menuSession === session ? null : session); }}
        role="button"
        aria-label={`Options for ${session}`}
      >
        &#x22EE;
      </span>
      {menuSession === session && (
        <div class="mobile-drawer-session-menu">
          <button onClick={() => handleRename(session)}>Rename</button>
          <button onClick={() => handleTag(session)}>Tags</button>
          <button class="destructive" onClick={() => handleKill(session)}>Delete</button>
        </div>
      )}
    </div>
  );

  return (
    <>
      <div class={`mobile-drawer-backdrop ${closing ? "closing" : ""}`} onClick={onClose} />
      <nav class={`mobile-drawer ${closing ? "closing" : ""}`} ref={drawerRef} aria-label="Session navigation">
        <div class="mobile-drawer-header">
          <span class="mobile-drawer-title">wsh</span>
          <div class="mobile-drawer-connection">
            <span class={`status-dot ${connState}`} />
            <span>{connState}</span>
          </div>
        </div>

        <button class="mobile-drawer-new-session" onClick={handleNewSession}>
          + New Session
        </button>

        <div class="mobile-drawer-sessions" onClick={() => setMenuSession(null)}>
          {showFlat ? (
            allSessions.map(s => renderSession(s, "all"))
          ) : (
            displayGroups.map(group => (
              <div key={group.tag} class="mobile-drawer-group">
                <div class="mobile-drawer-group-header">{group.label}</div>
                {group.sessions.map(s => renderSession(s, group.tag))}
              </div>
            ))
          )}
        </div>

        <div class="mobile-drawer-footer">
          <ThemePicker />
          <div style={{ flex: 1 }} />
          <button
            class={`mobile-drawer-rotate-btn ${autoRotate ? "on" : ""}`}
            onClick={toggleAutoRotate}
            aria-label={autoRotate ? "Lock portrait" : "Enable auto-rotate"}
            title={autoRotate ? "Auto-rotate: On" : "Auto-rotate: Off"}
          >
            <svg width="20" height="20" viewBox="0 0 20 20" fill="none" aria-hidden="true">
              <rect x="5" y="2" width="10" height="16" rx="2" stroke="currentColor" stroke-width="1.5" fill="none" />
              <path d="M2 7a7 7 0 0 1 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" fill="none" />
              <path d="M1 5l1.5 2 2-1.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round" fill="none" />
            </svg>
          </button>
        </div>
      </nav>
    </>
  );
}
