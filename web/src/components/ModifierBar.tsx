import { useRef, useEffect, useCallback, useState } from "preact/hooks";
import { ctrlActive, altActive, toggleCtrl, toggleAlt } from "../state/modifiers";
import { connectionState } from "../state/sessions";
import type { WshClient } from "../api/ws";

interface ModifierBarProps {
  session: string;
  client: WshClient;
  onTabSent?: () => void;
}

interface KeyDef {
  label: string;
  seq: string | null;
  modifier?: "ctrl" | "alt";
  repeatable?: boolean;
}

const KEYS: KeyDef[] = [
  { label: "Tab", seq: "\t" },
  { label: "Esc", seq: "\x1b" },
  { label: "Ctrl", seq: null, modifier: "ctrl" },
  { label: "Alt", seq: null, modifier: "alt" },
  { label: "Enter", seq: "\r" },
  { label: "Bksp", seq: "\x7f", repeatable: true },
  { label: "\u2190", seq: "\x1b[D", repeatable: true },
  { label: "\u2192", seq: "\x1b[C", repeatable: true },
  { label: "\u2191", seq: "\x1b[A", repeatable: true },
  { label: "\u2193", seq: "\x1b[B", repeatable: true },
  { label: "Home", seq: "\x1b[H" },
  { label: "End", seq: "\x1b[F" },
  { label: "PgUp", seq: "\x1b[5~" },
  { label: "PgDn", seq: "\x1b[6~" },
  { label: "F1", seq: "\x1bOP" },
  { label: "F2", seq: "\x1bOQ" },
  { label: "F3", seq: "\x1bOR" },
  { label: "F4", seq: "\x1bOS" },
  { label: "F5", seq: "\x1b[15~" },
  { label: "F6", seq: "\x1b[17~" },
  { label: "F7", seq: "\x1b[18~" },
  { label: "F8", seq: "\x1b[19~" },
  { label: "F9", seq: "\x1b[20~" },
  { label: "F10", seq: "\x1b[21~" },
  { label: "F11", seq: "\x1b[23~" },
  { label: "F12", seq: "\x1b[24~" },
];

const REPEAT_DELAY = 500;
const REPEAT_INTERVAL = 100;

export function ModifierBar({ session, client, onTabSent }: ModifierBarProps) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const repeatTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const repeatIntervalRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const didRepeatRef = useRef(false);
  const connected = connectionState.value === "connected";

  useEffect(() => {
    return () => {
      if (repeatTimerRef.current) clearTimeout(repeatTimerRef.current);
      if (repeatIntervalRef.current) clearInterval(repeatIntervalRef.current);
    };
  }, []);

  const [overflowLeft, setOverflowLeft] = useState(false);
  const [overflowRight, setOverflowRight] = useState(false);

  const updateOverflow = useCallback(() => {
    const el = wrapRef.current;
    if (!el) return;
    setOverflowLeft(el.scrollLeft > 0);
    setOverflowRight(el.scrollLeft + el.clientWidth < el.scrollWidth - 1);
  }, []);

  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    updateOverflow();
    el.addEventListener("scroll", updateOverflow, { passive: true });
    return () => el.removeEventListener("scroll", updateOverflow);
  }, [updateOverflow]);

  const send = useCallback(
    (data: string) => {
      if (!connected) return;
      client.sendInput(session, data).catch((e) => {
        console.error(`ModifierBar: failed to send input:`, e);
      });
    },
    [connected, client, session],
  );

  const handleTap = useCallback(
    (key: KeyDef) => {
      // On touch devices, touchstart handlers already performed the
      // action. Suppress the synthetic click that follows.
      if (didRepeatRef.current) {
        didRepeatRef.current = false;
        return;
      }
      if (key.modifier === "ctrl") {
        toggleCtrl();
        return;
      }
      if (key.modifier === "alt") {
        toggleAlt();
        return;
      }
      if (key.seq) {
        send(key.seq);
        if (key.seq === "\t" && onTabSent) onTabSent();
      }
    },
    [send, onTabSent],
  );

  const preventFocusSteal = useCallback((e: Event) => {
    e.preventDefault();
  }, []);

  const handleTouchStart = useCallback(
    (key: KeyDef, e: TouchEvent) => {
      e.preventDefault(); // prevent focus steal from input
      didRepeatRef.current = true;
      if (key.modifier === "ctrl") {
        toggleCtrl();
      } else if (key.modifier === "alt") {
        toggleAlt();
      } else if (key.seq) {
        send(key.seq);
        if (key.seq === "\t" && onTabSent) onTabSent();
      }
    },
    [send, onTabSent],
  );

  const startRepeat = useCallback(
    (key: KeyDef, e: TouchEvent) => {
      e.preventDefault(); // prevent focus steal from input
      if (!key.repeatable || !key.seq) return;
      const seq = key.seq;
      // Send the initial key immediately and mark that touch handled it,
      // so the synthetic onClick that follows touchend is suppressed.
      didRepeatRef.current = true;
      send(seq);
      repeatTimerRef.current = setTimeout(() => {
        repeatIntervalRef.current = setInterval(() => send(seq), REPEAT_INTERVAL);
      }, REPEAT_DELAY);
    },
    [send],
  );

  const stopRepeat = useCallback(() => {
    if (repeatTimerRef.current) {
      clearTimeout(repeatTimerRef.current);
      repeatTimerRef.current = null;
    }
    if (repeatIntervalRef.current) {
      clearInterval(repeatIntervalRef.current);
      repeatIntervalRef.current = null;
    }
  }, []);

  const wrapClass =
    "modifier-bar-wrap" +
    (overflowLeft ? " overflow-left" : "") +
    (overflowRight ? " overflow-right" : "");

  return (
    <div class={wrapClass}>
      <div class="modifier-bar" ref={wrapRef}>
        {KEYS.map((key) => {
          const isActive =
            (key.modifier === "ctrl" && ctrlActive.value) ||
            (key.modifier === "alt" && altActive.value);
          return (
            <button
              key={key.label}
              class={`modifier-key${isActive ? " active" : ""}`}
              disabled={!connected}
              onMouseDown={preventFocusSteal}
              onTouchStart={key.repeatable ? (e: TouchEvent) => startRepeat(key, e) : (e: TouchEvent) => handleTouchStart(key, e)}
              onClick={() => handleTap(key)}
              onTouchEnd={key.repeatable ? stopRepeat : undefined}
              onTouchCancel={key.repeatable ? stopRepeat : undefined}
            >
              {key.label}
            </button>
          );
        })}
      </div>
    </div>
  );
}
