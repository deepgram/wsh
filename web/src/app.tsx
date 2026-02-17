import { useEffect, useRef } from "preact/hooks";
import { WshClient } from "./api/ws";
import {
  sessions,
  focusedSession,
  sessionOrder,
  viewMode,
  tileLayout,
  tileSelection,
  connectionState,
  theme,
} from "./state/sessions";
import {
  setFullScreen,
  updateScreen,
  updateLine,
  removeScreen,
  getScreen,
} from "./state/terminal";
import { SessionCarousel } from "./components/SessionCarousel";
import { SessionGrid } from "./components/SessionGrid";
import { TiledLayout } from "./components/TiledLayout";
import { StatusBar } from "./components/StatusBar";

// Track unsubscribe functions for per-session subscriptions
const unsubscribes = new Map<string, () => void>();

export function App() {
  const clientRef = useRef<WshClient | null>(null);

  useEffect(() => {
    const client = new WshClient();
    clientRef.current = client;

    client.onStateChange = (state) => {
      connectionState.value = state;
      if (state === "connected") {
        initSessions(client);
      }
    };

    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    client.connect(`${proto}//${location.host}/ws/json`);

    return () => {
      client.disconnect();
    };
  }, []);

  // Keyboard shortcut for overview toggle
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key === "o") {
        e.preventDefault();
        viewMode.value = viewMode.value === "overview" ? "focused" : "overview";
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, []);

  // Sync theme class to <html>
  const currentTheme = theme.value;
  useEffect(() => {
    const root = document.documentElement;
    root.classList.remove("theme-glass", "theme-neon", "theme-minimal");
    root.classList.add(`theme-${currentTheme}`);
  }, [currentTheme]);

  // Read connectionState to subscribe to changes (re-render when client connects)
  const _connState = connectionState.value;
  const mode = viewMode.value;
  const client = clientRef.current;

  if (!client) {
    return (
      <>
        <div class="loading">Connecting...</div>
        <StatusBar client={null} />
      </>
    );
  }

  return (
    <>
      {mode === "focused" && <SessionCarousel client={client} />}
      {mode === "overview" && <SessionGrid client={client} />}
      {mode === "tiled" && <TiledLayout client={client} />}
      <StatusBar client={client} />
    </>
  );
}

async function initSessions(client: WshClient): Promise<void> {
  try {
    // Clean up old subscriptions from previous connection
    for (const unsub of unsubscribes.values()) unsub();
    unsubscribes.clear();
    client.clearAllSubscriptions();

    const list = await client.listSessions();
    let names = list.map((s) => s.name);

    if (names.length === 0) {
      const created = await client.createSession();
      names = [created.name];
    }

    sessions.value = names;
    sessionOrder.value = [...names];

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
  });

  const unsub = client.subscribe(
    name,
    ["lines", "cursor", "mode"],
    (event) => {
      handleEvent(name, event);
    },
  );
  unsubscribes.set(name, unsub);
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function handleLifecycleEvent(client: WshClient, raw: any): void {
  switch (raw.event) {
    case "session_created": {
      const name = raw.params?.name;
      if (!name || sessions.value.includes(name)) break;
      sessions.value = [...sessions.value, name];
      sessionOrder.value = [...sessionOrder.value, name];
      setupSession(client, name).catch((e) => {
        console.error(`Failed to set up new session "${name}":`, e);
      });
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
      if (focusedSession.value === name) {
        focusedSession.value = sessionOrder.value[0] ?? null;
      }
      // Clean up tile selection
      if (tileSelection.value.includes(name)) {
        tileSelection.value = tileSelection.value.filter((s) => s !== name);
      }
      // Clean up tile layout
      if (tileLayout.value?.sessions.includes(name)) {
        const remaining = tileLayout.value.sessions.filter((s) => s !== name);
        if (remaining.length < 2) {
          tileLayout.value = null;
          viewMode.value = "focused";
        } else {
          const evenSize = 1 / remaining.length;
          tileLayout.value = {
            sessions: remaining,
            sizes: remaining.map(() => evenSize),
          };
        }
      }
      break;
    }

    case "session_exited": {
      // Process exited but session object still exists
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

      const screenState = getScreen(oldName);
      removeScreen(oldName);
      setFullScreen(newName, screenState);

      const unsub = unsubscribes.get(oldName);
      if (unsub) {
        unsub();
        unsubscribes.delete(oldName);
      }
      setupSession(client, newName).catch((e) => {
        console.error(`Failed to set up renamed session "${newName}":`, e);
      });

      if (focusedSession.value === oldName) {
        focusedSession.value = newName;
      }
      if (tileLayout.value?.sessions.includes(oldName)) {
        tileLayout.value = {
          ...tileLayout.value,
          sessions: tileLayout.value.sessions.map((s) =>
            s === oldName ? newName : s,
          ),
        };
      }
      break;
    }
  }
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function handleEvent(session: string, raw: any): void {
  switch (raw.event) {
    case "sync":
    case "diff": {
      const screen = raw.params?.screen ?? raw.screen;
      if (!screen) break;
      setFullScreen(session, {
        lines: screen.lines,
        cursor: screen.cursor,
        alternateActive: screen.alternate_active,
        cols: screen.cols,
        rows: screen.rows,
        firstLineIndex: screen.first_line_index,
      });
      break;
    }

    case "line":
      updateLine(session, raw.index, raw.line);
      break;

    case "cursor":
      updateScreen(session, {
        cursor: { row: raw.row, col: raw.col, visible: raw.visible },
      });
      break;

    case "mode":
      updateScreen(session, { alternateActive: raw.alternate_active });
      break;
  }
}
