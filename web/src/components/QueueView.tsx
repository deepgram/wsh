import { useEffect, useRef, useState } from "preact/hooks";
import { signal } from "@preact/signals";
import { WshClient } from "../api/ws";
import { OrchestratorClient } from "../api/orchestrator";
import type { QueueEntry } from "../api/orchestrator";
import { setFullScreen, updateLine, updateScreen } from "../state/terminal";
import { connectionState, theme } from "../state/sessions";
import { CardStack } from "./CardStack";

// Track which sessions we've set up (subscribed to events, fetched screen)
const subscribedSessions = new Set<string>();
const unsubscribes = new Map<string, () => void>();

// Queue entries signal for reactive rendering
const queueEntries = signal<QueueEntry[]>([]);
const orchConnected = signal(false);

export function QueueView() {
  const wshClientRef = useRef<WshClient | null>(null);
  const orchClientRef = useRef<OrchestratorClient | null>(null);

  // Connect to both wsh and orchestrator
  useEffect(() => {
    const wshClient = new WshClient();
    wshClientRef.current = wshClient;

    wshClient.onStateChange = (state) => {
      connectionState.value = state;
    };

    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    wshClient.connect(`${proto}//${location.host}/ws/json`);

    const orchClient = new OrchestratorClient("/orch-api");
    orchClientRef.current = orchClient;

    orchClient.onChange = (entries) => {
      queueEntries.value = [...entries];
      // Ensure we have terminal subscriptions for all queue sessions
      for (const entry of entries) {
        ensureSessionSetup(wshClient, entry.session_name);
      }
    };

    orchClient.onConnectionChange = (connected) => {
      orchConnected.value = connected;
    };

    orchClient.connect();

    return () => {
      wshClient.disconnect();
      orchClient.disconnect();
      for (const unsub of unsubscribes.values()) unsub();
      unsubscribes.clear();
      subscribedSessions.clear();
    };
  }, []);

  // Sync theme class to <html>
  const currentTheme = theme.value;
  useEffect(() => {
    const root = document.documentElement;
    root.classList.remove("theme-glass", "theme-neon", "theme-minimal");
    root.classList.add(`theme-${currentTheme}`);
  }, [currentTheme]);

  const entries = queueEntries.value;
  const wshClient = wshClientRef.current;
  const _connState = connectionState.value;
  const _orchConn = orchConnected.value;

  const handleResolve = async (entryId: string, action: string, text?: string) => {
    const orchClient = orchClientRef.current;
    if (!orchClient) return;
    try {
      await orchClient.resolve(entryId, action, text);
      // Optimistically remove from local state
      queueEntries.value = queueEntries.value.filter((e) => e.id !== entryId);
    } catch (e) {
      console.error("Failed to resolve:", e);
    }
  };

  return (
    <div class="queue-view">
      <div class="queue-header">
        <a href="/" class="queue-back-link">&larr; Terminal</a>
        <div class="queue-title">Triage Queue</div>
        <div class="queue-status">
          {entries.length > 0 && (
            <span class="queue-count">{entries.length} waiting</span>
          )}
          <span
            class={`queue-dot ${orchConnected.value ? "connected" : "disconnected"}`}
          />
        </div>
      </div>

      {!wshClient ? (
        <div class="loading">Connecting...</div>
      ) : (
        <CardStack
          entries={entries}
          wshClient={wshClient}
          onResolve={handleResolve}
        />
      )}
    </div>
  );
}

async function ensureSessionSetup(client: WshClient, sessionName: string): Promise<void> {
  if (subscribedSessions.has(sessionName)) return;
  subscribedSessions.add(sessionName);

  try {
    const screen = await client.getScreen(sessionName, "styled");
    setFullScreen(sessionName, {
      lines: screen.lines,
      cursor: screen.cursor,
      alternateActive: screen.alternate_active,
      cols: screen.cols,
      rows: screen.rows,
      firstLineIndex: screen.first_line_index,
    });

    const unsub = client.subscribe(sessionName, ["lines", "cursor", "mode"], (event) => {
      handleTerminalEvent(sessionName, event);
    });
    unsubscribes.set(sessionName, unsub);
  } catch (e) {
    console.error(`Failed to set up session "${sessionName}" for queue:`, e);
    subscribedSessions.delete(sessionName);
  }
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function handleTerminalEvent(session: string, raw: any): void {
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
