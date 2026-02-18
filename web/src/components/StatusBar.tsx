import { useRef, useEffect } from "preact/hooks";
import {
  connectionState,
  focusedSession,
  viewMode,
  theme,
  cycleTheme,
} from "../state/sessions";
import type { WshClient } from "../api/ws";

interface StatusBarProps {
  client: WshClient | null;
}

export function StatusBar({ client }: StatusBarProps) {
  const connected = connectionState.value;
  const session = focusedSession.value;
  const mode = viewMode.value;
  const currentTheme = theme.value;
  const toastRef = useRef<HTMLSpanElement>(null);
  const toastTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Clean up timer on unmount
  useEffect(() => {
    return () => {
      if (toastTimer.current) clearTimeout(toastTimer.current);
    };
  }, []);

  const toggleOverview = () => {
    viewMode.value = mode === "overview" ? "focused" : "overview";
  };

  const handleNewSession = async () => {
    if (!client || connected !== "connected") return;
    try {
      const created = await client.createSession();
      focusedSession.value = created.name;
    } catch (e) {
      console.error("Failed to create session:", e);
    }
  };

  const handleThemeCycle = () => {
    const next = cycleTheme();
    // Flash theme name
    const el = toastRef.current;
    if (el) {
      el.textContent = next;
      el.classList.remove("theme-toast-visible");
      // Force reflow to restart animation
      void el.offsetWidth;
      el.classList.add("theme-toast-visible");
      if (toastTimer.current) clearTimeout(toastTimer.current);
      toastTimer.current = setTimeout(() => {
        el.classList.remove("theme-toast-visible");
      }, 1500);
    }
  };

  return (
    <div class="status-bar">
      <div class="status-bar-left">
        <div class={`status-dot ${connected}`} />
        <span>
          {connected === "connected"
            ? session ?? "no session"
            : connected}
        </span>
        <span ref={toastRef} class="theme-toast" />
      </div>
      <div class="status-bar-right">
        {connected === "connected" && (
          <>
            <button
              class="status-btn"
              onClick={handleThemeCycle}
              title={`Theme: ${currentTheme}`}
            >
              <svg width="14" height="14" viewBox="0 0 14 14">
                <circle
                  cx="7"
                  cy="7"
                  r="5"
                  stroke="currentColor"
                  stroke-width="1.5"
                  fill="none"
                />
                <path
                  d="M7 2a5 5 0 0 1 0 10"
                  fill="currentColor"
                />
              </svg>
            </button>
            <button
              class="status-btn"
              onClick={handleNewSession}
              title="New session"
            >
              <svg width="14" height="14" viewBox="0 0 14 14">
                <path
                  d="M7 2v10M2 7h10"
                  stroke="currentColor"
                  stroke-width="1.5"
                  stroke-linecap="round"
                />
              </svg>
            </button>
            <button
              class={`overview-toggle ${mode === "overview" ? "active" : ""}`}
              onClick={toggleOverview}
              title="Session overview (Ctrl+Shift+O)"
            >
              <svg width="14" height="14" viewBox="0 0 14 14">
                <rect x="0" y="0" width="6" height="6" fill="currentColor" rx="1" />
                <rect x="8" y="0" width="6" height="6" fill="currentColor" rx="1" />
                <rect x="0" y="8" width="6" height="6" fill="currentColor" rx="1" />
                <rect x="8" y="8" width="6" height="6" fill="currentColor" rx="1" />
              </svg>
            </button>
            <a
              href="#/queue"
              class="status-btn"
              title="Triage queue"
            >
              <svg width="14" height="14" viewBox="0 0 14 14">
                <rect x="1" y="1" width="12" height="3" rx="1" fill="currentColor" />
                <rect x="2" y="5.5" width="10" height="3" rx="1" fill="currentColor" opacity="0.5" />
                <rect x="3" y="10" width="8" height="3" rx="1" fill="currentColor" opacity="0.25" />
              </svg>
            </a>
          </>
        )}
      </div>
    </div>
  );
}
