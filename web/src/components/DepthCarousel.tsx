import { useCallback, useEffect, useState } from "preact/hooks";
import type { WshClient } from "../api/ws";
import { focusedSession } from "../state/sessions";
import { SessionPane } from "./SessionPane";

interface DepthCarouselProps {
  sessions: string[];
  client: WshClient;
}

export function DepthCarousel({ sessions, client }: DepthCarouselProps) {
  const focused = focusedSession.value;
  // Find the index of the focused session, or default to 0
  const currentIndex = Math.max(0, sessions.indexOf(focused ?? ""));
  const [isMobile, setIsMobile] = useState(false);

  // Detect mobile viewport
  useEffect(() => {
    const mq = window.matchMedia("(max-width: 640px)");
    setIsMobile(mq.matches);
    const handler = (e: MediaQueryListEvent) => setIsMobile(e.matches);
    mq.addEventListener("change", handler);
    return () => mq.removeEventListener("change", handler);
  }, []);

  const navigate = useCallback((direction: -1 | 1) => {
    const newIndex = (currentIndex + direction + sessions.length) % sessions.length;
    focusedSession.value = sessions[newIndex];
  }, [currentIndex, sessions]);

  // Keyboard navigation: Ctrl+Shift+Left/Right
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (!e.ctrlKey || !e.shiftKey) return;
      if (e.altKey || e.metaKey) return;

      if (e.key === "ArrowLeft") {
        e.preventDefault();
        navigate(-1);
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        navigate(1);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [navigate]);

  if (sessions.length === 0) return null;

  // Single session -- just show it full-size
  if (sessions.length === 1) {
    return (
      <div class="depth-carousel">
        <div class="carousel-track">
          <div class="carousel-slide carousel-center">
            <SessionPane session={sessions[0]} client={client} />
          </div>
        </div>
      </div>
    );
  }

  // Mobile: full-width single session with navigation hints
  if (isMobile) {
    return (
      <div class="depth-carousel mobile">
        <div class="carousel-track">
          <div class="carousel-slide carousel-center">
            <SessionPane session={sessions[currentIndex]} client={client} />
          </div>
        </div>
        {sessions.length > 1 && (
          <div class="carousel-nav-hints">
            <button class="carousel-nav-btn" onClick={() => navigate(-1)}>&#9664;</button>
            <span class="carousel-position">{currentIndex + 1} / {sessions.length}</span>
            <button class="carousel-nav-btn" onClick={() => navigate(1)}>&#9654;</button>
          </div>
        )}
      </div>
    );
  }

  // Desktop: 3D depth effect
  const prevIndex = (currentIndex - 1 + sessions.length) % sessions.length;
  const nextIndex = (currentIndex + 1) % sessions.length;

  // 2-session case: only show center + one adjacent to avoid ghost overlap
  if (sessions.length === 2) {
    const otherIndex = currentIndex === 0 ? 1 : 0;
    return (
      <div class="depth-carousel">
        <div class="carousel-track">
          <div key={sessions[otherIndex]} class="carousel-slide carousel-prev" onClick={() => navigate(-1)}>
            <SessionPane session={sessions[otherIndex]} client={client} />
          </div>
          <div key={sessions[currentIndex]} class="carousel-slide carousel-center">
            <SessionPane session={sessions[currentIndex]} client={client} />
            <div class="carousel-focus-ring" />
          </div>
        </div>
      </div>
    );
  }

  // 3+ sessions: full 3D depth with prev + center + next
  return (
    <div class="depth-carousel">
      <div class="carousel-track">
        <div key={sessions[prevIndex]} class="carousel-slide carousel-prev" onClick={() => navigate(-1)}>
          <SessionPane session={sessions[prevIndex]} client={client} />
        </div>
        <div key={sessions[currentIndex]} class="carousel-slide carousel-center">
          <SessionPane session={sessions[currentIndex]} client={client} />
          <div class="carousel-focus-ring" />
        </div>
        <div key={sessions[nextIndex]} class="carousel-slide carousel-next" onClick={() => navigate(1)}>
          <SessionPane session={sessions[nextIndex]} client={client} />
        </div>
      </div>
    </div>
  );
}
