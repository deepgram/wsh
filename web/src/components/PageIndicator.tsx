import { focusedSession } from "../state/sessions";

interface PageIndicatorProps {
  sessions: string[];
  focused: string | null;
}

export function PageIndicator({ sessions, focused }: PageIndicatorProps) {
  if (sessions.length <= 1) return null;

  // Collapse to counter format if many sessions
  if (sessions.length > 8) {
    const idx = focused ? sessions.indexOf(focused) + 1 : 0;
    return (
      <div class="page-indicator">
        <span class="page-counter">
          {idx}/{sessions.length}
        </span>
      </div>
    );
  }

  return (
    <div class="page-indicator">
      {sessions.map((name) => (
        <div
          key={name}
          class={`page-dot ${name === focused ? "active" : ""}`}
          onClick={() => {
            focusedSession.value = name;
          }}
        />
      ))}
    </div>
  );
}
