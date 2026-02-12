import { useEffect, useRef } from "preact/hooks";
import { WshClient } from "./api/ws";
import { sessions, activeSession, connectionState } from "./state/sessions";
import { setFullScreen, updateScreen, updateLine } from "./state/terminal";
import { Terminal } from "./components/Terminal";
import { InputBar } from "./components/InputBar";

export function App() {
  const clientRef = useRef<WshClient | null>(null);

  useEffect(() => {
    const client = new WshClient();
    clientRef.current = client;

    client.onStateChange = (state) => {
      connectionState.value = state;

      if (state === "connected") {
        initSession(client);
      }
    };

    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    client.connect(`${proto}//${location.host}/ws/json`);

    return () => {
      client.disconnect();
    };
  }, []);

  return (
    <>
      <Terminal />
      <div class="status-bar">
        <div class={`status-dot ${connectionState.value}`} />
        <span>
          {connectionState.value === "connected"
            ? activeSession.value ?? "no session"
            : connectionState.value}
        </span>
      </div>
      {clientRef.current && <InputBar client={clientRef.current} />}
    </>
  );
}

async function initSession(client: WshClient): Promise<void> {
  try {
    const list = await client.listSessions();
    const names = list.map((s) => s.name);
    sessions.value = names;

    let target: string;
    if (names.length > 0) {
      target = names[0];
    } else {
      const created = await client.createSession();
      target = created.name;
      sessions.value = [target];
    }

    activeSession.value = target;

    const screen = await client.getScreen(target, "styled");
    setFullScreen(target, {
      lines: screen.lines,
      cursor: screen.cursor,
      alternateActive: screen.alternate_active,
      cols: screen.cols,
      rows: screen.rows,
      firstLineIndex: screen.first_line_index,
    });

    client.subscribe(target, ["lines", "cursor", "mode"], (event) => {
      handleEvent(target, event);
    });
  } catch (e) {
    console.error("Failed to initialize session:", e);
  }
}

// Server events use two formats:
// - sync/diff: payload nested in `params` (params.screen, etc.)
// - line/cursor/mode: payload at top level
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function handleEvent(session: string, raw: any): void {
  switch (raw.event) {
    case "sync":
    case "diff": {
      const screen = raw.params?.screen;
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
