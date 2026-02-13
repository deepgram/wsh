import { useRef, useEffect } from "preact/hooks";
import { focusedSession, connectionState, viewMode } from "../state/sessions";
import { getScreen } from "../state/terminal";
import type { WshClient } from "../api/ws";
import type { FormattedLine } from "../api/types";

interface InputBarProps {
  session: string;
  client: WshClient;
}

// Map key events to terminal escape sequences
function keyToSequence(e: KeyboardEvent): string | null {
  // Ctrl combos
  if (e.ctrlKey && !e.altKey && !e.metaKey) {
    const key = e.key.toLowerCase();
    if (key.length === 1 && key >= "a" && key <= "z") {
      return String.fromCharCode(key.charCodeAt(0) - 96); // Ctrl+A = 0x01, etc.
    }
    if (key === "[") return "\x1b";
    if (key === "\\") return "\x1c";
    if (key === "]") return "\x1d";
    return null;
  }

  // Alt combos — send ESC prefix
  if (e.altKey && !e.ctrlKey && !e.metaKey) {
    if (e.key.length === 1) {
      return "\x1b" + e.key;
    }
  }

  switch (e.key) {
    case "Enter":
      return "\r";
    case "Backspace":
      return "\x7f";
    case "Tab":
      return "\t";
    case "Escape":
      return "\x1b";
    case "ArrowUp":
      return "\x1b[A";
    case "ArrowDown":
      return "\x1b[B";
    case "ArrowRight":
      return "\x1b[C";
    case "ArrowLeft":
      return "\x1b[D";
    case "Home":
      return "\x1b[H";
    case "End":
      return "\x1b[F";
    case "PageUp":
      return "\x1b[5~";
    case "PageDown":
      return "\x1b[6~";
    case "Insert":
      return "\x1b[2~";
    case "Delete":
      return "\x1b[3~";
    case "F1":
      return "\x1bOP";
    case "F2":
      return "\x1bOQ";
    case "F3":
      return "\x1bOR";
    case "F4":
      return "\x1bOS";
    case "F5":
      return "\x1b[15~";
    case "F6":
      return "\x1b[17~";
    case "F7":
      return "\x1b[18~";
    case "F8":
      return "\x1b[19~";
    case "F9":
      return "\x1b[20~";
    case "F10":
      return "\x1b[21~";
    case "F11":
      return "\x1b[23~";
    case "F12":
      return "\x1b[24~";
    default:
      return null;
  }
}

function lineToPlainText(line: FormattedLine): string {
  if (typeof line === "string") return line;
  return line.map((span) => span.text).join("");
}

export function InputBar({ session, client }: InputBarProps) {
  const inputRef = useRef<HTMLInputElement>(null);
  const prevValueRef = useRef("");
  const pendingRef = useRef<{ promptLen: number } | null>(null);
  const syncTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const connected = connectionState.value === "connected";

  // Auto-focus on desktop when this session becomes focused
  const isFocused = session === focusedSession.value;
  useEffect(() => {
    if (
      isFocused &&
      viewMode.value === "focused" &&
      window.matchMedia("(pointer: fine)").matches
    ) {
      inputRef.current?.focus();
    }
  }, [isFocused]);

  const send = (data: string) => {
    if (!connected) return;
    client.sendInput(session, data).catch((e) => {
      console.error(`Failed to send input to session "${session}":`, e);
    });
  };

  const clearInput = () => {
    const input = inputRef.current;
    if (input) {
      input.value = "";
      prevValueRef.current = "";
    }
  };

  const resolveCompletion = () => {
    const pending = pendingRef.current;
    pendingRef.current = null;
    if (!pending) return;

    const screen = getScreen(session);
    const { row: cursorRow, col: cursorCol } = screen.cursor;

    if (cursorRow >= 0 && cursorRow < screen.lines.length && pending.promptLen >= 0) {
      const text = lineToPlainText(screen.lines[cursorRow]);
      if (pending.promptLen <= cursorCol) {
        const input = inputRef.current;
        if (input) {
          input.value = text.slice(pending.promptLen, cursorCol);
          prevValueRef.current = input.value;
        }
        return;
      }
    }

    clearInput();
  };

  const scheduleSyncFromTerminal = () => {
    const screen = getScreen(session);
    const inputLen = inputRef.current?.value.length ?? 0;
    pendingRef.current = { promptLen: screen.cursor.col - inputLen };
    if (syncTimerRef.current) clearTimeout(syncTimerRef.current);
    syncTimerRef.current = setTimeout(resolveCompletion, 150);
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    const seq = keyToSequence(e);
    if (seq !== null) {
      e.preventDefault();
      send(seq);

      const input = inputRef.current;
      if (input) {
        if (e.key === "Enter" || e.key === "Escape") {
          clearInput();
        } else if (e.key === "Tab" || e.key === "ArrowUp" || e.key === "ArrowDown") {
          scheduleSyncFromTerminal();
        } else if (e.key === "Backspace") {
          // Remove last char from visual buffer
          input.value = input.value.slice(0, -1);
          prevValueRef.current = input.value;
        }
        // For other control sequences (arrows, Ctrl+X, Tab, etc.),
        // keep the visual buffer as-is
      }
      return;
    }

    // Printable characters fall through to handleInput
  };

  const handleInput = () => {
    const input = inputRef.current;
    if (!input) return;

    const current = input.value;
    const prev = prevValueRef.current;

    if (current.length > prev.length) {
      // New text added — send only the new characters
      send(current.slice(prev.length));
    } else if (current.length < prev.length && current.length === 0) {
      // Full clear (e.g. autocomplete replacement) — ignore, already handled
    }

    prevValueRef.current = current;
  };

  return (
    <div class="input-bar">
      <input
        ref={inputRef}
        type="text"
        placeholder={connected ? "Type here..." : "Disconnected"}
        disabled={!connected}
        onKeyDown={handleKeyDown}
        onInput={handleInput}
        autocomplete="off"
        autocapitalize="off"
        autocorrect="off"
        spellcheck={false}
      />
    </div>
  );
}
