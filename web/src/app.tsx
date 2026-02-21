import { useEffect, useRef, useState } from "preact/hooks";
import { useCallback } from "preact/hooks";
import { WshClient } from "./api/ws";
import {
  sessions,
  focusedSession,
  sessionOrder,
  connectionState,
  theme,
  authToken,
  authRequired,
  authError,
  sessionInfoMap,
  sidebarCollapsed,
} from "./state/sessions";
import type { SessionInfo } from "./api/types";
import {
  setFullScreen,
  updateScreen,
  updateLine,
  removeScreen,
  getScreen,
} from "./state/terminal";
import { selectedGroups, groups, activeGroupSessions, sessionStatuses } from "./state/groups";
import { LayoutShell } from "./components/LayoutShell";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { CommandPalette } from "./components/CommandPalette";
import { ShortcutSheet } from "./components/ShortcutSheet";

// Track unsubscribe functions for per-session subscriptions
const unsubscribes = new Map<string, () => void>();

function TokenPrompt({ client }: { client: WshClient }) {
  const error = authError.value;
  const hasStoredToken = !!authToken.value;

  const handleSubmit = useCallback(
    (e: Event) => {
      e.preventDefault();
      const form = e.target as HTMLFormElement;
      const input = form.elements.namedItem("token") as HTMLInputElement;
      const token = input.value.trim();
      if (!token) return;

      localStorage.setItem("wsh-auth-token", token);
      authToken.value = token;
      authError.value = null;
      authRequired.value = false;
      client.setToken(token);
      client.disconnect();
      const proto = location.protocol === "https:" ? "wss:" : "ws:";
      client.connect(`${proto}//${location.host}/ws/json`);
    },
    [client],
  );

  const handleClear = useCallback(() => {
    localStorage.removeItem("wsh-auth-token");
    authToken.value = null;
    authError.value = null;
  }, []);

  return (
    <div class="auth-prompt-backdrop">
      <form class="auth-prompt" onSubmit={handleSubmit}>
        <div class="auth-prompt-title">Authentication Required</div>
        {error && <div class="auth-prompt-error">{error}</div>}
        <div class="auth-prompt-desc">
          This server requires an auth token. Run{" "}
          <code>wsh token</code> to retrieve it.
        </div>
        <input
          name="token"
          type="password"
          class="auth-prompt-input"
          placeholder="Paste token here"
          autoFocus
        />
        <button type="submit" class="auth-prompt-btn">
          Connect
        </button>
        {hasStoredToken && (
          <button type="button" class="auth-prompt-clear" onClick={handleClear}>
            Clear saved token
          </button>
        )}
      </form>
    </div>
  );
}

export function App() {
  const clientRef = useRef<WshClient | null>(null);

  useEffect(() => {
    const client = new WshClient();
    clientRef.current = client;

    // Set token from localStorage before connecting
    if (authToken.value) {
      client.setToken(authToken.value);
    }

    client.onStateChange = (state) => {
      connectionState.value = state;
      if (state === "connected") {
        initSessions(client);
      }
    };

    client.onAuthRequired = (reason) => {
      if (reason === "invalid") {
        authError.value = "Invalid token. Please try again.";
      } else {
        authError.value = null;
      }
      authRequired.value = true;
    };

    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    client.connect(`${proto}//${location.host}/ws/json`);

    return () => {
      client.disconnect();
    };
  }, []);

  // Command palette state
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [shortcutSheetOpen, setShortcutSheetOpen] = useState(false);

  // Global keyboard shortcuts: Ctrl+Shift+X
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (!e.ctrlKey || !e.shiftKey) return;
      if (e.altKey || e.metaKey) return;

      const key = e.key;

      // Command palette
      if (key === "k" || key === "K") {
        e.preventDefault();
        setPaletteOpen((v) => !v);
        return;
      }
      // Shortcut help
      if (key === "/" || key === "?") {
        e.preventDefault();
        setShortcutSheetOpen((v) => !v);
        return;
      }
      // Toggle sidebar
      if (key === "b" || key === "B") {
        e.preventDefault();
        sidebarCollapsed.value = !sidebarCollapsed.value;
        localStorage.setItem("wsh-sidebar-collapsed", String(sidebarCollapsed.value));
        return;
      }
      // New session
      if (key === "o" || key === "O") {
        e.preventDefault();
        clientRef.current?.createSession().catch(() => {});
        return;
      }
      // Kill focused session
      if (key === "w" || key === "W") {
        e.preventDefault();
        const focused = focusedSession.value;
        if (focused && confirm(`Kill session "${focused}"?`)) {
          clientRef.current?.killSession(focused).catch(() => {});
        }
        return;
      }
      // Jump to Nth session (Ctrl+Shift+1-9)
      if (key >= "1" && key <= "9") {
        e.preventDefault();
        const idx = parseInt(key) - 1;
        const groupSessions = activeGroupSessions.value;
        if (idx < groupSessions.length) {
          focusedSession.value = groupSessions[idx];
        }
        return;
      }
      // Cycle sidebar groups
      if (key === "Tab") {
        e.preventDefault();
        const allGroups = groups.value;
        if (allGroups.length === 0) return;
        const current = selectedGroups.value[0] || "all";
        const currentIdx = allGroups.findIndex((g) => g.tag === current);
        const direction = 1;
        const nextIdx = (currentIdx + direction + allGroups.length) % allGroups.length;
        selectedGroups.value = [allGroups[nextIdx].tag];
        return;
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, []);

  // Sync theme class to <html>
  const currentTheme = theme.value;
  useEffect(() => {
    const root = document.documentElement;
    root.className = ""; // clear all classes
    root.classList.add(`theme-${currentTheme}`);
  }, [currentTheme]);

  // Read connectionState to subscribe to changes (re-render when client connects)
  const _connState = connectionState.value;
  const needsAuth = authRequired.value;
  const client = clientRef.current;

  if (!client) {
    return (
      <div class="loading">Connecting...</div>
    );
  }

  if (needsAuth) {
    return <TokenPrompt client={client} />;
  }

  return (
    <ErrorBoundary>
      <LayoutShell client={client} />
      {paletteOpen && (
        <CommandPalette client={client} onClose={() => setPaletteOpen(false)} />
      )}
      {shortcutSheetOpen && (
        <ShortcutSheet onClose={() => setShortcutSheetOpen(false)} />
      )}
    </ErrorBoundary>
  );
}

async function initSessions(client: WshClient): Promise<void> {
  try {
    // Clean up old subscriptions from previous connection
    for (const unsub of unsubscribes.values()) unsub();
    unsubscribes.clear();
    client.clearAllSubscriptions();

    let infos = await client.listSessions();

    if (infos.length === 0) {
      const created = await client.createSession();
      infos = [created];
    }

    const names = infos.map((s) => s.name);
    sessions.value = names;
    sessionOrder.value = [...names];

    // Populate session info map with tag data
    const infoMap = new Map<string, SessionInfo>();
    for (const info of infos) {
      infoMap.set(info.name, info);
    }
    sessionInfoMap.value = infoMap;

    if (!focusedSession.value || !names.includes(focusedSession.value)) {
      focusedSession.value = names[0];
    }

    // Fetch all screens and subscribe in parallel
    await Promise.all(names.map((name) => setupSession(client, name)));

    // Set up lifecycle event handler
    client.onLifecycleEvent = (event) => handleLifecycleEvent(client, event);
  } catch (e) {
    console.error("Failed to initialize sessions:", e);
  }
}

async function setupSession(
  client: WshClient,
  name: string,
): Promise<void> {
  const screen = await client.getScreen(name, "styled");
  setFullScreen(name, {
    lines: screen.lines,
    cursor: screen.cursor,
    alternateActive: screen.alternate_active,
    cols: screen.cols,
    rows: screen.rows,
    firstLineIndex: screen.first_line_index,
    totalLines: screen.total_lines,
    scrollbackLines: [],
    scrollbackOffset: 0,
    scrollbackComplete: false,
    scrollbackLoading: false,
  });

  const unsub = client.subscribe(
    name,
    ["lines", "cursor", "mode", "activity"],
    (event) => {
      // Use event.session if available (handles renames without re-subscribe)
      const target = (event.session as string) ?? name;
      handleEvent(client, target, event);
    },
    2000, // idle_timeout_ms
  );
  unsubscribes.set(name, unsub);
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function handleLifecycleEvent(client: WshClient, raw: any): void {
  switch (raw.event) {
    case "session_created": {
      const name = raw.params?.name;
      if (!name) break;
      if (!sessions.value.includes(name)) {
        sessions.value = [...sessions.value, name];
        sessionOrder.value = [...sessionOrder.value, name];
      }
      // Update session info map
      const createdMap = new Map(sessionInfoMap.value);
      createdMap.set(name, {
        name,
        pid: raw.params?.pid ?? null,
        command: raw.params?.command ?? "",
        rows: raw.params?.rows ?? 24,
        cols: raw.params?.cols ?? 80,
        clients: raw.params?.clients ?? 0,
        tags: raw.params?.tags ?? [],
      });
      sessionInfoMap.value = createdMap;
      // Always set up screen state and subscription if not already subscribed
      if (!unsubscribes.has(name)) {
        setupSession(client, name).catch((e) => {
          console.error(`Failed to set up new session "${name}":`, e);
        });
      }
      break;
    }

    case "session_destroyed": {
      const name = raw.params?.name;
      if (!name) break;
      const unsub = unsubscribes.get(name);
      if (unsub) {
        unsub();
        unsubscribes.delete(name);
      }
      removeScreen(name);
      sessions.value = sessions.value.filter((s) => s !== name);
      sessionOrder.value = sessionOrder.value.filter((s) => s !== name);
      // Remove from session info map
      const destroyedMap = new Map(sessionInfoMap.value);
      destroyedMap.delete(name);
      sessionInfoMap.value = destroyedMap;
      if (focusedSession.value === name) {
        focusedSession.value = sessionOrder.value[0] ?? null;
      }
      break;
    }

    case "session_renamed": {
      const oldName = raw.params?.old_name;
      const newName = raw.params?.new_name;
      if (!oldName || !newName) break;

      sessions.value = sessions.value.map((s) =>
        s === oldName ? newName : s,
      );
      sessionOrder.value = sessionOrder.value.map((s) =>
        s === oldName ? newName : s,
      );

      // Update session info map key
      const renamedMap = new Map(sessionInfoMap.value);
      const renamedInfo = renamedMap.get(oldName);
      if (renamedInfo) {
        renamedMap.delete(oldName);
        renamedMap.set(newName, { ...renamedInfo, name: newName });
      }
      sessionInfoMap.value = renamedMap;

      // Move screen state from old signal to new signal
      const screenState = getScreen(oldName);
      removeScreen(oldName);
      setFullScreen(newName, screenState);

      // Re-key subscriptions — no unsubscribe/resubscribe, no event gap.
      // The server already updated its forwarding task name.
      const unsub = unsubscribes.get(oldName);
      if (unsub) {
        unsubscribes.delete(oldName);
        unsubscribes.set(newName, unsub);
      }
      client.rekeySubscription(oldName, newName);

      if (focusedSession.value === oldName) {
        focusedSession.value = newName;
      }
      break;
    }

    case "session_tags_changed": {
      const taggedName = raw.params?.name;
      const tags = raw.params?.tags;
      if (!taggedName || !tags) break;
      const taggedMap = new Map(sessionInfoMap.value);
      const taggedInfo = taggedMap.get(taggedName);
      if (taggedInfo) {
        taggedMap.set(taggedName, { ...taggedInfo, tags });
        sessionInfoMap.value = taggedMap;
      }
      break;
    }
  }
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function handleEvent(client: WshClient, session: string, raw: any): void {
  switch (raw.event) {
    case "sync":
    case "diff": {
      const screen = raw.params?.screen ?? raw.screen;
      if (!screen) break;
      // Preserve existing scrollback cache across sync/diff updates
      const current = getScreen(session);
      const newTotalLines = screen.total_lines ?? current.totalLines;
      const newAvailable = Math.max(0, newTotalLines - screen.rows);
      // Reset scrollbackComplete when new unfetched scrollback becomes available
      const complete = current.scrollbackComplete && newAvailable > current.scrollbackOffset
        ? false
        : current.scrollbackComplete;
      setFullScreen(session, {
        lines: screen.lines,
        cursor: screen.cursor,
        alternateActive: screen.alternate_active,
        cols: screen.cols,
        rows: screen.rows,
        firstLineIndex: screen.first_line_index,
        totalLines: newTotalLines,
        scrollbackLines: current.scrollbackLines,
        scrollbackOffset: current.scrollbackOffset,
        scrollbackComplete: complete,
        scrollbackLoading: current.scrollbackLoading,
      });
      break;
    }

    case "line":
      updateLine(session, raw.index, raw.line);
      if (raw.total_lines !== undefined) {
        const current = getScreen(session);
        const newAvailable = Math.max(0, raw.total_lines - current.rows);
        // Reset scrollbackComplete when new unfetched scrollback becomes available
        const resetComplete = current.scrollbackComplete && newAvailable > current.scrollbackOffset;
        updateScreen(session, {
          totalLines: raw.total_lines,
          ...(resetComplete ? { scrollbackComplete: false } : {}),
        });
      }
      break;

    case "cursor":
      updateScreen(session, {
        cursor: { row: raw.row, col: raw.col, visible: raw.visible },
      });
      break;

    case "mode":
      updateScreen(session, { alternateActive: raw.alternate_active });
      break;

    case "reset":
      // Re-fetch full screen state after reset (resize, clear, alt screen, parser restart)
      client.getScreen(session, "styled").then((screen) => {
        setFullScreen(session, {
          lines: screen.lines,
          cursor: screen.cursor,
          alternateActive: screen.alternate_active,
          cols: screen.cols,
          rows: screen.rows,
          firstLineIndex: screen.first_line_index,
          totalLines: screen.total_lines,
          scrollbackLines: [],
          scrollbackOffset: 0,
          scrollbackComplete: false,
          scrollbackLoading: false,
        });
      }).catch((e) => {
        console.error(`Failed to re-fetch screen after reset for "${session}":`, e);
      });
      break;

    case "idle": {
      const updated = new Map(sessionStatuses.value);
      updated.set(session, "idle");
      sessionStatuses.value = updated;
      break;
    }

    case "running": {
      const updated = new Map(sessionStatuses.value);
      updated.set(session, "running");
      sessionStatuses.value = updated;
      break;
    }
  }
}
