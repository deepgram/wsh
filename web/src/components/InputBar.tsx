import { useRef, useEffect } from "preact/hooks";
import { focusedSession, connectionState } from "../state/sessions";
import { getScreen } from "../state/terminal";
import { ctrlActive, altActive, clearModifiers } from "../state/modifiers";
import type { WshClient } from "../api/ws";
import { keyToSequence, lineToPlainText } from "../utils/keymap";

interface InputBarProps {
  session: string;
  client: WshClient;
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
      window.matchMedia("(pointer: fine)").matches
    ) {
      inputRef.current?.focus();
    }
  }, [isFocused]);

  // Clean up sync timer on unmount
  useEffect(() => {
    return () => {
      if (syncTimerRef.current) {
        clearTimeout(syncTimerRef.current);
        syncTimerRef.current = null;
      }
    };
  }, []);

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

  // Mobile virtual keyboards often bypass keydown for Enter, firing
  // a beforeinput with inputType "insertLineBreak" instead. Intercept
  // it here so Enter reliably sends \r to the PTY.
  const handleBeforeInput = (e: InputEvent) => {
    if (e.inputType === "insertLineBreak" || e.inputType === "insertParagraph") {
      e.preventDefault();
      send("\r");
      clearInput();
    }
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
          // Remove last code point from visual buffer (handles multi-byte chars)
          const chars = Array.from(input.value);
          chars.pop();
          input.value = chars.join("");
          prevValueRef.current = input.value;
        }
        // For other control sequences (Ctrl+X, etc.), keep the visual buffer as-is
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

    if (current === prev) return;

    // If a modifier is active, intercept the newly typed character(s)
    if (ctrlActive.value || altActive.value) {
      const added = current.slice(prev.length);
      if (added.length === 1) {
        if (ctrlActive.value) {
          const lower = added.toLowerCase();
          if (lower >= "a" && lower <= "z") {
            send(String.fromCharCode(lower.charCodeAt(0) - 96));
          }
        } else if (altActive.value) {
          send("\x1b" + added);
        }
        clearModifiers();
        // Revert the input field — the modified char shouldn't appear
        input.value = prev;
        prevValueRef.current = prev;
        return;
      }
    }

    // Normal diff-based input (no modifier active)
    // Find common prefix
    let common = 0;
    while (common < prev.length && common < current.length && prev[common] === current[common]) {
      common++;
    }

    // Characters removed from prev after the common prefix
    const removed = prev.length - common;
    // Characters added in current after the common prefix
    const added = current.slice(common);

    if (removed > 0) {
      send("\x7f".repeat(removed));
    }
    if (added) {
      send(added);
    }

    prevValueRef.current = current;
  };

  return (
    <div class={`input-bar${ctrlActive.value || altActive.value ? " modifier-active" : ""}`}>
      <input
        ref={inputRef}
        type="text"
        enterkeyhint="send"
        placeholder={
          !connected
            ? "Disconnected"
            : ctrlActive.value
              ? "Ctrl + ..."
              : altActive.value
                ? "Alt + ..."
                : "Type here..."
        }
        disabled={!connected}
        onBeforeInput={handleBeforeInput}
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
