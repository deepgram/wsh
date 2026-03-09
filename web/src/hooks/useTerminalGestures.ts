import { useEffect } from "preact/hooks";
import type { RefObject } from "preact";

/** Minimum px distance before a swipe fires. */
const SWIPE_THRESHOLD = 30;
/** px of movement before axis is locked. */
const AXIS_LOCK_THRESHOLD = 15;

interface GestureOptions {
  /** Ref to the element to attach touch listeners to. */
  containerRef: RefObject<HTMLElement>;
  /** Called with the ANSI sequence for the detected swipe direction. */
  onSwipe: (seq: string) => void;
  /** Whether the hook is enabled (false on desktop). */
  enabled: boolean;
}

export function useTerminalGestures({
  containerRef,
  onSwipe,
  enabled,
}: GestureOptions): void {
  useEffect(() => {
    if (!enabled) return;
    const el = containerRef.current;
    if (!el) return;

    let tracking = false;
    let originX = 0;
    let originY = 0;
    let lockedAxis: "h" | "v" | null = null;
    let fired = false;

    let touch0Start: { x: number; y: number } | null = null;
    let touch1Start: { x: number; y: number } | null = null;

    function midpoint(t: TouchList): { x: number; y: number } {
      return {
        x: (t[0].clientX + t[1].clientX) / 2,
        y: (t[0].clientY + t[1].clientY) / 2,
      };
    }

    function isPinch(t: TouchList): boolean {
      if (!touch0Start || !touch1Start) return false;
      const d0x = t[0].clientX - touch0Start.x;
      const d0y = t[0].clientY - touch0Start.y;
      const d1x = t[1].clientX - touch1Start.x;
      const d1y = t[1].clientY - touch1Start.y;
      const dot = d0x * d1x + d0y * d1y;
      const mag0 = Math.sqrt(d0x * d0x + d0y * d0y);
      const mag1 = Math.sqrt(d1x * d1x + d1y * d1y);
      if (mag0 < 5 || mag1 < 5) return false;
      return dot / (mag0 * mag1) < 0.3;
    }

    const onTouchStart = (e: TouchEvent) => {
      if (e.touches.length === 2) {
        const mid = midpoint(e.touches);
        originX = mid.x;
        originY = mid.y;
        touch0Start = { x: e.touches[0].clientX, y: e.touches[0].clientY };
        touch1Start = { x: e.touches[1].clientX, y: e.touches[1].clientY };
        tracking = true;
        lockedAxis = null;
        fired = false;
      }
    };

    const onTouchMove = (e: TouchEvent) => {
      if (!tracking || e.touches.length !== 2) {
        tracking = false;
        return;
      }
      if (fired) return;
      if (isPinch(e.touches)) {
        tracking = false;
        return;
      }

      const mid = midpoint(e.touches);
      const dx = mid.x - originX;
      const dy = mid.y - originY;
      const absDx = Math.abs(dx);
      const absDy = Math.abs(dy);

      if (!lockedAxis && (absDx > AXIS_LOCK_THRESHOLD || absDy > AXIS_LOCK_THRESHOLD)) {
        lockedAxis = absDx > absDy ? "h" : "v";
      }

      if (!lockedAxis) return;

      if (lockedAxis === "h" && absDx > SWIPE_THRESHOLD) {
        onSwipe(dx < 0 ? "\x1b[D" : "\x1b[C");
        fired = true;
      } else if (lockedAxis === "v" && absDy > SWIPE_THRESHOLD) {
        onSwipe(dy < 0 ? "\x1b[A" : "\x1b[B");
        fired = true;
      }
    };

    const onTouchEnd = () => {
      tracking = false;
      lockedAxis = null;
      touch0Start = null;
      touch1Start = null;
    };

    el.addEventListener("touchstart", onTouchStart, { passive: true });
    el.addEventListener("touchmove", onTouchMove, { passive: true });
    el.addEventListener("touchend", onTouchEnd);
    el.addEventListener("touchcancel", onTouchEnd);

    return () => {
      el.removeEventListener("touchstart", onTouchStart);
      el.removeEventListener("touchmove", onTouchMove);
      el.removeEventListener("touchend", onTouchEnd);
      el.removeEventListener("touchcancel", onTouchEnd);
    };
  }, [containerRef, onSwipe, enabled]);
}
