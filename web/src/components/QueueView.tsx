import { useEffect, useRef } from "preact/hooks";
import { WshClient } from "../api/ws";
import {
  sessions,
  sessionOrder,
  focusedSession,
  connectionState,
  authToken,
  authRequired,
  authError,
  theme,
} from "../state/sessions";
import {
  setFullScreen,
  updateScreen,
  updateLine,
  removeScreen,
  getScreen,
} from "../state/terminal";
import { QueueDetector, queueEntries } from "../queue/detector";
import { CardStack } from "./CardStack";

// Per-session unsubscribe functions (scoped to this view's lifetime)
const unsubscribes = new Map<string, () => void>();

export function QueueView() {
  const clientRef = useRef<WshClient | null>(null);
  const detectorRef = useRef<QueueDetector | null>(null);

  useEffect(() => {
    const client = new WshClient();
    clientRef.current = client;

    if (authToken.value) {
      client.setToken(authToken.value);
    }

    client.onStateChange = (state) => {
      connectionState.value = state;
      if (state === "connected") {
        initQueueSessions(client).then(() => {
          if (!detectorRef.current) {
            const detector = new QueueDetector();
            detectorRef.current = detector;
            detector.start();
          }
        });
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
      detectorRef.current?.stop();
      detectorRef.current = null;
      for (const unsub of unsubscribes.values()) unsub();
      unsubscribes.clear();
      client.disconnect();
      clientRef.current = null;
    };
  }, []);

  // Sync theme class
  const currentTheme = theme.value;
  useEffect(() => {
    const root = document.documentElement;
    root.classList.remove("theme-glass", "theme-neon", "theme-minimal");
    root.classList.add(`theme-${currentTheme}`);
  }, [currentTheme]);

  const entries = queueEntries.value;
  const connected = connectionState.value;
  const client = clientRef.current;

  const handleDismiss = (sessionName: string) => {
    detectorRef.current?.dismiss(sessionName);
  };

  if (!client || connected !== "connected") {
    return (
      <div class="queue-view">
        <div class="queue-header">
          <a class="queue-back" href="#/">
            <svg width="14" height="14" viewBox="0 0 14 14">
              <path
                d="M9 2L4 7l5 5"
                stroke="currentColor"
                stroke-width="1.5"
                fill="none"
                stroke-linecap="round"
                stroke-linejoin="round"
              />
            </svg>
          </a>
          <span class="queue-title">Queue</span>
        </div>
        <div class="loading">Connecting...</div>
      </div>
    );
  }

  return (
    <div class="queue-view">
      <div class="queue-header">
        <a class="queue-back" href="#/">
          <svg width="14" height="14" viewBox="0 0 14 14">
            <path
              d="M9 2L4 7l5 5"
              stroke="currentColor"
              stroke-width="1.5"
              fill="none"
              stroke-linecap="round"
              stroke-linejoin="round"
            />
          </svg>
        </a>
        <span class="queue-title">Queue</span>
        <span class="queue-count">{entries.length}</span>
      </div>
      <CardStack entries={entries} client={client} onDismiss={handleDismiss} />
    </div>
  );
}

// --- Session initialization (mirrors app.tsx logic for this view) ---

async function initQueueSessions(client: WshClient): Promise<void> {
  try {
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

    await Promise.all(names.map((name) => setupQueueSession(client, name)));

    client.onLifecycleEvent = (event) =>
      handleQueueLifecycle(client, event);
  } catch (e) {
    console.error("Failed to initialize queue sessions:", e);
  }
}

async function setupQueueSession(
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
      const target = (event.session as string) ?? name;
      handleQueueEvent(client, target, event);
    },
  );
  unsubscribes.set(name, unsub);
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function handleQueueLifecycle(client: WshClient, raw: any): void {
  switch (raw.event) {
    case "session_created": {
      const name = raw.params?.name;
      if (!name || sessions.value.includes(name)) break;
      sessions.value = [...sessions.value, name];
      sessionOrder.value = [...sessionOrder.value, name];
      setupQueueSession(client, name).catch((e) => {
        console.error(`Failed to set up session "${name}":`, e);
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
        unsubscribes.delete(oldName);
        unsubscribes.set(newName, unsub);
      }
      client.rekeySubscription(oldName, newName);

      if (focusedSession.value === oldName) {
        focusedSession.value = newName;
      }
      break;
    }
  }
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function handleQueueEvent(client: WshClient, session: string, raw: any): void {
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

    case "reset":
      client
        .getScreen(session, "styled")
        .then((screen) => {
          setFullScreen(session, {
            lines: screen.lines,
            cursor: screen.cursor,
            alternateActive: screen.alternate_active,
            cols: screen.cols,
            rows: screen.rows,
            firstLineIndex: screen.first_line_index,
          });
        })
        .catch((e) => {
          console.error(
            `Failed to re-fetch screen after reset for "${session}":`,
            e,
          );
        });
      break;
  }
}
