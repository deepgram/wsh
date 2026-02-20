import { useCallback, useEffect, useRef, useState } from "preact/hooks";
import type { WshClient } from "../api/ws";
import { focusedSession } from "../state/sessions";
import { MiniTermContent } from "./MiniViewPreview";
import { SessionPane } from "./SessionPane";

interface DepthCarouselProps {
  sessions: string[];
  client: WshClient;
}

export function DepthCarousel({ sessions, client }: DepthCarouselProps) {
  const focused = focusedSession.value;
  const currentIndex = Math.max(0, sessions.indexOf(focused ?? ""));
  const [isMobile, setIsMobile] = useState(false);
  const stripRef = useRef<HTMLDivElement>(null);

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

  // Scroll the film strip to center the active thumb
  useEffect(() => {
    const strip = stripRef.current;
    if (!strip) return;
    const activeThumb = strip.children[currentIndex] as HTMLElement | undefined;
    if (activeThumb) {
      const stripRect = strip.getBoundingClientRect();
      const thumbRect = activeThumb.getBoundingClientRect();
      const scrollLeft = activeThumb.offsetLeft - (stripRect.width / 2) + (thumbRect.width / 2);
      strip.scrollTo({ left: scrollLeft, behavior: "smooth" });
    }
  }, [currentIndex]);

  // Check if strip overflows (for fade indicators)
  const [overflowLeft, setOverflowLeft] = useState(false);
  const [overflowRight, setOverflowRight] = useState(false);

  const updateOverflow = useCallback(() => {
    const strip = stripRef.current;
    if (!strip) return;
    setOverflowLeft(strip.scrollLeft > 4);
    setOverflowRight(strip.scrollLeft + strip.clientWidth < strip.scrollWidth - 4);
  }, []);

  useEffect(() => {
    const strip = stripRef.current;
    if (!strip) return;
    updateOverflow();
    strip.addEventListener("scroll", updateOverflow, { passive: true });
    const ro = new ResizeObserver(updateOverflow);
    ro.observe(strip);
    return () => {
      strip.removeEventListener("scroll", updateOverflow);
      ro.disconnect();
    };
  }, [updateOverflow, sessions.length]);

  if (sessions.length === 0) return null;

  // Single session — no strip needed
  if (sessions.length === 1) {
    return (
      <div class="depth-carousel">
        <div class="carousel-main">
          <SessionPane session={sessions[0]} client={client} />
        </div>
      </div>
    );
  }

  // Mobile: full-width single session with navigation hints
  if (isMobile) {
    return (
      <div class="depth-carousel mobile">
        <div class="carousel-main">
          <SessionPane session={sessions[currentIndex]} client={client} />
        </div>
        <div class="carousel-nav-hints">
          <button class="carousel-nav-btn" onClick={() => navigate(-1)}>&#9664;</button>
          <span class="carousel-position">{currentIndex + 1} / {sessions.length}</span>
          <button class="carousel-nav-btn" onClick={() => navigate(1)}>&#9654;</button>
        </div>
      </div>
    );
  }

  // Desktop: film strip + full-width active session
  return (
    <div class="depth-carousel">
      <div class={`carousel-strip-wrap ${overflowLeft ? "overflow-left" : ""} ${overflowRight ? "overflow-right" : ""}`}>
        <div class="carousel-strip" ref={stripRef}>
          {sessions.map((s, i) => (
            <div
              key={s}
              class={`carousel-thumb ${i === currentIndex ? "active" : ""}`}
              onClick={() => { focusedSession.value = s; }}
            >
              <MiniTermContent session={s} />
              <div class="carousel-thumb-label">{s}</div>
            </div>
          ))}
        </div>
      </div>
      <div class="carousel-main">
        <SessionPane session={sessions[currentIndex]} client={client} />
      </div>
    </div>
  );
}
