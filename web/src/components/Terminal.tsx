import { useRef, useEffect, useCallback } from "preact/hooks";
import { getScreenSignal, updateScreen } from "../state/terminal";
import { connectionState, zoomLevel } from "../state/sessions";
import { spanStyle } from "../utils/terminal";
import type { WshClient } from "../api/ws";
import type { FormattedLine } from "../api/types";

/** How many scrollback lines to fetch per page. */
const SCROLLBACK_PAGE_SIZE = 200;

/** Trigger scrollback fetch when scrollTop is within this many px of top. */
const SCROLLBACK_THRESHOLD = 100;

function renderLine(
  line: FormattedLine,
  lineIdx: number,
  cursor: { col: number } | null,
): preact.JSX.Element {
  if (typeof line === "string") {
    if (cursor !== null) {
      const before = line.slice(0, cursor.col);
      const cursorChar = line[cursor.col] || " ";
      const after = line.slice(cursor.col + 1);
      return (
        <div class="term-line" key={lineIdx}>
          {before}
          <span class="term-cursor">{cursorChar}</span>
          {after || null}
        </div>
      );
    }
    return (
      <div class="term-line" key={lineIdx}>
        {line || "\u00A0"}
      </div>
    );
  }

  // Styled spans — empty line
  if (line.length === 0) {
    if (cursor !== null) {
      return (
        <div class="term-line" key={lineIdx}>
          <span class="term-cursor">{" "}</span>
        </div>
      );
    }
    return (
      <div class="term-line" key={lineIdx}>
        {"\u00A0"}
      </div>
    );
  }

  // Styled spans — no cursor on this line
  if (cursor === null) {
    return (
      <div class="term-line" key={lineIdx}>
        {line.map((span, i) => (
          <span key={i} style={spanStyle(span)}>
            {span.text}
          </span>
        ))}
      </div>
    );
  }

  // Styled spans — cursor on this line, split at cursor.col
  const elements: preact.JSX.Element[] = [];
  let col = 0;
  let cursorRendered = false;

  for (let i = 0; i < line.length; i++) {
    const span = line[i];
    const spanEnd = col + span.text.length;

    if (!cursorRendered && cursor.col >= col && cursor.col < spanEnd) {
      const offset = cursor.col - col;
      const before = span.text.slice(0, offset);
      const cursorChar = span.text[offset] || " ";
      const after = span.text.slice(offset + 1);

      if (before) elements.push(<span key={`${i}a`} style={spanStyle(span)}>{before}</span>);
      elements.push(<span key={`${i}c`} class="term-cursor">{cursorChar}</span>);
      if (after) elements.push(<span key={`${i}b`} style={spanStyle(span)}>{after}</span>);
      cursorRendered = true;
    } else {
      elements.push(<span key={i} style={spanStyle(span)}>{span.text}</span>);
    }
    col = spanEnd;
  }

  if (!cursorRendered) {
    elements.push(<span key="cursor" class="term-cursor">{" "}</span>);
  }

  return (
    <div class="term-line" key={lineIdx}>
      {elements}
    </div>
  );
}

/** Base font size in px — zoom multiplies this. */
const BASE_FONT_SIZE = 14;

/** Debounce delay for resize events (ms). */
const RESIZE_DEBOUNCE_MS = 150;

interface TerminalProps {
  session: string;
  client?: WshClient;
}

export function Terminal({ session, client }: TerminalProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const measureRef = useRef<HTMLSpanElement>(null);
  const userScrolledRef = useRef(false);
  const resizeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const lastSizeRef = useRef<{ cols: number; rows: number } | null>(null);

  // Subscribe only to this session's signal (not all sessions)
  const screen = getScreenSignal(session).value;
  const disconnected = connectionState.value !== "connected";
  const zoom = zoomLevel.value;
  const fontSize = BASE_FONT_SIZE * zoom;

  // Measure character cell size and compute cols/rows for a given container size
  const computeGridSize = useCallback(() => {
    const measure = measureRef.current;
    const container = containerRef.current;
    if (!measure || !container) return null;

    const rect = measure.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return null;

    const cellWidth = rect.width;
    const cellHeight = rect.height;

    // Account for container padding (4px top/bottom, 8px left/right)
    const style = getComputedStyle(container);
    const padX = parseFloat(style.paddingLeft) + parseFloat(style.paddingRight);
    const padY = parseFloat(style.paddingTop) + parseFloat(style.paddingBottom);

    const cols = Math.floor((container.clientWidth - padX) / cellWidth);
    const rows = Math.floor((container.clientHeight - padY) / cellHeight);

    return { cols: Math.max(cols, 1), rows: Math.max(rows, 1) };
  }, []);

  // ResizeObserver — debounced resize sent to server
  useEffect(() => {
    if (!client) return;
    const container = containerRef.current;
    if (!container) return;

    const observer = new ResizeObserver(() => {
      if (resizeTimerRef.current) clearTimeout(resizeTimerRef.current);
      resizeTimerRef.current = setTimeout(() => {
        const size = computeGridSize();
        if (!size) return;
        const last = lastSizeRef.current;
        if (last && last.cols === size.cols && last.rows === size.rows) return;
        lastSizeRef.current = size;
        client.resize(session, size.cols, size.rows).catch(() => {});
      }, RESIZE_DEBOUNCE_MS);
    });

    observer.observe(container);
    return () => {
      observer.disconnect();
      if (resizeTimerRef.current) clearTimeout(resizeTimerRef.current);
    };
  }, [session, client, computeGridSize]);

  // Re-trigger resize measurement when zoom changes
  useEffect(() => {
    if (!client) return;
    // Small delay to let the browser reflow with new font size
    const timer = setTimeout(() => {
      const size = computeGridSize();
      if (!size) return;
      const last = lastSizeRef.current;
      if (last && last.cols === size.cols && last.rows === size.rows) return;
      lastSizeRef.current = size;
      client.resize(session, size.cols, size.rows).catch(() => {});
    }, 50);
    return () => clearTimeout(timer);
  }, [zoom, session, client, computeGridSize]);

  // Fetch scrollback when user scrolls near the top
  const fetchScrollback = useCallback(() => {
    if (!client) return;
    if (screen.scrollbackComplete || screen.scrollbackLoading) return;
    if (screen.alternateActive) return;

    // Derive scrollback available from totalLines and rows — these are kept
    // current by line events, unlike firstLineIndex which only updates on
    // sync/diff/reset.
    const scrollbackAvailable = Math.max(0, screen.totalLines - screen.rows);
    if (scrollbackAvailable <= 0) return;
    if (screen.scrollbackOffset >= scrollbackAvailable) {
      updateScreen(session, { scrollbackComplete: true });
      return;
    }

    updateScreen(session, { scrollbackLoading: true });

    // Fetch from the end of scrollback backwards
    const remaining = scrollbackAvailable - screen.scrollbackOffset;
    const limit = Math.min(SCROLLBACK_PAGE_SIZE, remaining);
    const offset = scrollbackAvailable - screen.scrollbackOffset - limit;

    client
      .getScrollback(session, Math.max(0, offset), limit)
      .then((resp) => {
        const sig = getScreenSignal(session);
        const current = sig.value;
        const currentAvailable = Math.max(0, current.totalLines - current.rows);

        // If the server returned 0 lines, our request was likely based on stale
        // state (e.g., a resize happened between request and response, absorbing
        // scrollback into the larger PTY).  Don't mark as complete — only set it
        // if the *current* state also shows no scrollback available.
        if (resp.lines.length === 0) {
          sig.value = {
            ...current,
            scrollbackLoading: false,
            scrollbackComplete: currentAvailable <= 0,
            totalLines: resp.total_lines,
          };
          return;
        }

        // Prepend new lines before existing scrollback
        const newScrollback = [...resp.lines, ...current.scrollbackLines];
        const newOffset = current.scrollbackOffset + resp.lines.length;
        const complete = newOffset >= currentAvailable || resp.lines.length < limit;
        sig.value = {
          ...current,
          scrollbackLines: newScrollback,
          scrollbackOffset: newOffset,
          scrollbackComplete: complete,
          scrollbackLoading: false,
          totalLines: resp.total_lines,
        };
      })
      .catch(() => {
        updateScreen(session, { scrollbackLoading: false });
      });
  }, [client, session, screen.scrollbackComplete, screen.scrollbackLoading, screen.alternateActive, screen.totalLines, screen.rows, screen.scrollbackOffset]);

  // Track manual scrolling + trigger scrollback fetch near top
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const handleScroll = () => {
      const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 30;
      userScrolledRef.current = !atBottom;

      // Load more scrollback when near the top
      if (el.scrollTop < SCROLLBACK_THRESHOLD) {
        fetchScrollback();
      }
    };
    // Wheel events fire even when there's no overflow (unlike scroll events).
    // This is critical: when the terminal content exactly fits the container
    // (server rows == visible rows), there's no scrollbar and scroll events
    // never fire. The user scrolling up with the wheel/trackpad should still
    // trigger scrollback loading.
    const handleWheel = (e: WheelEvent) => {
      if (e.deltaY < 0 && el.scrollTop === 0) {
        // Mark as user-scrolled so auto-scroll doesn't snap back to bottom
        // when scrollback lines are prepended and the component re-renders.
        userScrolledRef.current = true;
        fetchScrollback();
      }
    };
    el.addEventListener("scroll", handleScroll);
    el.addEventListener("wheel", handleWheel, { passive: true });
    return () => {
      el.removeEventListener("scroll", handleScroll);
      el.removeEventListener("wheel", handleWheel);
    };
  }, [fetchScrollback]);

  // Auto-scroll to bottom when new content arrives (only in normal mode, only if at bottom)
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    if (!screen.alternateActive && !userScrolledRef.current) {
      el.scrollTop = el.scrollHeight;
    }
  });

  const containerClass = screen.alternateActive
    ? "terminal-container alternate"
    : "terminal-container";

  // Cursor is relative to the screen lines (not scrollback)
  const scrollbackLen = screen.scrollbackLines.length;
  const cursorLineIndex = screen.cursor.visible
    ? scrollbackLen + screen.cursor.row
    : -1;

  // Combine scrollback + screen lines for rendering
  const allLines: FormattedLine[] = screen.alternateActive
    ? screen.lines
    : [...screen.scrollbackLines, ...screen.lines];

  return (
    <div
      class={containerClass}
      ref={containerRef}
      style={{ fontSize: `${fontSize}px` }}
    >
      {/* Hidden measurement span for character cell size */}
      <span
        ref={measureRef}
        style={{
          position: "absolute",
          visibility: "hidden",
          whiteSpace: "pre",
          fontFamily: "inherit",
          fontSize: "inherit",
          lineHeight: "inherit",
        }}
      >
        X
      </span>
      {allLines.map((line, i) =>
        renderLine(line, i, i === cursorLineIndex ? { col: screen.cursor.col } : null),
      )}
      {disconnected && (
        <div class="terminal-disconnected">Connection lost</div>
      )}
    </div>
  );
}
