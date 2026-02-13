import { useRef, useEffect, useCallback } from "preact/hooks";
import { sessionOrder, focusedSession, viewMode } from "../state/sessions";
import { SessionPane } from "./SessionPane";
import { PageIndicator } from "./PageIndicator";
import type { WshClient } from "../api/ws";

interface SessionCarouselProps {
  client: WshClient;
}

export function SessionCarousel({ client }: SessionCarouselProps) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const order = sessionOrder.value;
  const focused = focusedSession.value;

  // Scroll to focused session when it changes programmatically
  useEffect(() => {
    if (!focused || !scrollRef.current) return;
    const idx = order.indexOf(focused);
    if (idx < 0) return;
    const container = scrollRef.current;
    const target = idx * container.clientWidth;
    if (Math.abs(container.scrollLeft - target) > 1) {
      container.scrollTo({ left: target, behavior: "smooth" });
    }
  }, [focused, order]);

  // Update focusedSession on scroll end
  useEffect(() => {
    const container = scrollRef.current;
    if (!container) return;

    const onScrollEnd = () => {
      if (!container.clientWidth) return;
      const idx = Math.round(container.scrollLeft / container.clientWidth);
      const session = order[idx];
      if (session && session !== focusedSession.value) {
        focusedSession.value = session;
      }
    };

    // Use scrollend where available, fall back to debounced scroll
    let timer: ReturnType<typeof setTimeout> | null = null;
    const supportsScrollEnd =
      "onscrollend" in HTMLElement.prototype;

    if (supportsScrollEnd) {
      container.addEventListener("scrollend", onScrollEnd);
    } else {
      const onScroll = () => {
        if (timer) clearTimeout(timer);
        timer = setTimeout(onScrollEnd, 150);
      };
      container.addEventListener("scroll", onScroll);
      return () => {
        container.removeEventListener("scroll", onScroll);
        if (timer) clearTimeout(timer);
      };
    }

    return () => {
      if (supportsScrollEnd) {
        container.removeEventListener("scrollend", onScrollEnd);
      }
    };
  }, [order]);

  // Swipe-up gesture detection for overview mode
  const touchStartRef = useRef<{ x: number; y: number } | null>(null);

  const handleTouchStart = useCallback((e: TouchEvent) => {
    const touch = e.touches[0];
    // Only detect swipe-up from bottom 40px of viewport
    if (touch.clientY >= window.innerHeight - 40) {
      touchStartRef.current = { x: touch.clientX, y: touch.clientY };
    } else {
      touchStartRef.current = null;
    }
  }, []);

  const handleTouchEnd = useCallback((e: TouchEvent) => {
    const start = touchStartRef.current;
    touchStartRef.current = null;
    if (!start) return;
    const touch = e.changedTouches[0];
    const dy = start.y - touch.clientY; // positive = upward
    const dx = Math.abs(touch.clientX - start.x);
    if (dy > 80 && dy > dx) {
      viewMode.value = "overview";
    }
  }, []);

  return (
    <div
      class="carousel-wrapper"
      onTouchStart={handleTouchStart}
      onTouchEnd={handleTouchEnd}
    >
      <div class="session-carousel" ref={scrollRef}>
        {order.map((name) => (
          <SessionPane key={name} session={name} client={client} />
        ))}
      </div>
      <PageIndicator sessions={order} focused={focused} />
    </div>
  );
}
