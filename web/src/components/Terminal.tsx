import { useRef, useEffect, useCallback, useState } from "preact/hooks";
import { memo } from "preact/compat";
import { useTerminalGestures } from "../hooks/useTerminalGestures";
import { getScreenSignal, updateScreen } from "../state/terminal";
import { connectionState, focusedSession, zoomLevel } from "../state/sessions";
import { spanStyle } from "../utils/terminal";
import { keyToSequence } from "../utils/keymap";
import type { WshClient } from "../api/ws";
import type { FormattedLine } from "../api/types";
import { OverlayLayer } from "./OverlayLayer";
import { PanelRegion, computePanelLayout } from "./PanelRegion";

/** How many scrollback lines to fetch per page. */
const SCROLLBACK_PAGE_SIZE = 200;

/** Trigger scrollback fetch when scrollTop is within this many px of top. */
const SCROLLBACK_THRESHOLD = 100;

/** Lines to render above/below the visible viewport for smooth scrolling. */
const OVERSCAN = 20;

// ---------------------------------------------------------------------------
// Memoized line component — skips VDOM diffing when the line ref is unchanged
// ---------------------------------------------------------------------------

interface MemoLineProps {
  line: FormattedLine;
  lineIdx: number;
  cursor: { col: number } | null;
}

const MemoLine = memo(function MemoLine({ line, lineIdx, cursor }: MemoLineProps) {
  return renderLine(line, lineIdx, cursor);
});

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
  captureInput?: boolean;
}

/**
 * Index-based line accessor — avoids creating a new
 * [...scrollbackLines, ...lines] array on every render.
 */
function getLine(
  scrollbackLines: FormattedLine[],
  screenLines: FormattedLine[],
  index: number,
): FormattedLine {
  if (index < scrollbackLines.length) {
    return scrollbackLines[index];
  }
  return screenLines[index - scrollbackLines.length];
}

export function Terminal({ session, client, captureInput }: TerminalProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const measureRef = useRef<HTMLSpanElement>(null);
  const userScrolledRef = useRef(false);
  const resizeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const lastSizeRef = useRef<{ cols: number; rows: number } | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const [cellSize, setCellSize] = useState<{ w: number; h: number } | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const prevScrollbackLenRef = useRef(0);

  // Subscribe only to this session's signal (not all sessions)
  const screen = getScreenSignal(session).value;
  const disconnected = connectionState.value !== "connected";
  const zoom = zoomLevel.value;
  const fontSize = BASE_FONT_SIZE * zoom;

  // Line height: use measured cell height, fall back to fontSize * 1.4
  const lineHeight = cellSize ? cellSize.h : fontSize * 1.4;

  // Total line count (scrollback + screen) — without allocating an array
  const scrollbackLen = screen.scrollbackLines.length;
  const totalLines = screen.alternateActive
    ? screen.lines.length
    : scrollbackLen + screen.lines.length;

  // Adjust scrollTop after scrollback prepend to maintain visual position
  useEffect(() => {
    const prevLen = prevScrollbackLenRef.current;
    const curLen = screen.scrollbackLines.length;
    if (curLen > prevLen) {
      const el = containerRef.current;
      if (el) {
        const delta = (curLen - prevLen) * lineHeight;
        el.scrollTop += delta;
      }
    }
    prevScrollbackLenRef.current = curLen;
  }, [screen.scrollbackLines.length, lineHeight]);

  // Auto-focus textarea when this session is focused (desktop input capture)
  const isFocused = session === focusedSession.value;
  useEffect(() => {
    if (captureInput && isFocused && textareaRef.current) {
      textareaRef.current.focus({ preventScroll: true });
    }
  }, [captureInput, isFocused]);

  // Two-finger swipe gestures for arrow keys on mobile
  const handleSwipe = useCallback(
    (seq: string) => {
      if (client) {
        client.sendInput(session, seq).catch(() => {});
      }
    },
    [client, session],
  );

  useTerminalGestures({
    containerRef,
    onSwipe: handleSwipe,
    enabled: !captureInput && !!client,
  });

  // Handle keyboard input from hidden textarea
  const handleTextareaKeyDown = useCallback((e: KeyboardEvent) => {
    if (!client) return;
    // Let Ctrl+Shift combos bubble up for UI shortcuts
    if (e.ctrlKey && e.shiftKey) return;
    const seq = keyToSequence(e);
    if (seq !== null) {
      e.preventDefault();
      client.sendInput(session, seq).catch(() => {});
    }
  }, [client, session]);

  // Handle text input (printable characters, IME, paste) from hidden textarea
  const handleTextareaInput = useCallback(() => {
    if (!client) return;
    const ta = textareaRef.current;
    if (!ta) return;
    const value = ta.value;
    if (value) {
      client.sendInput(session, value).catch(() => {});
      ta.value = "";
    }
  }, [client, session]);

  // Click on terminal wrapper focuses the hidden textarea
  const handleWrapperClick = useCallback(() => {
    if (captureInput && textareaRef.current) {
      textareaRef.current.focus({ preventScroll: true });
    }
  }, [captureInput]);

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

    return { cols: Math.max(cols, 1), rows: Math.max(rows, 1), cellWidth, cellHeight };
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
        setCellSize({ w: size.cellWidth, h: size.cellHeight });
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
      setCellSize({ w: size.cellWidth, h: size.cellHeight });
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

    // On the first fetch, snapshot totalLines as the anchor for pagination.
    // Subsequent fetches reuse this anchor so that new output arriving between
    // fetches doesn't shift offsets and cause gaps or overlaps.
    const anchorTotal = screen.scrollbackAnchorTotalLines ?? screen.totalLines;
    const scrollbackAvailable = Math.max(0, anchorTotal - screen.rows);
    if (scrollbackAvailable <= 0) return;
    if (screen.scrollbackOffset >= scrollbackAvailable) {
      updateScreen(session, { scrollbackComplete: true });
      return;
    }

    updateScreen(session, {
      scrollbackLoading: true,
      // Persist the anchor on the first fetch
      ...(screen.scrollbackAnchorTotalLines == null
        ? { scrollbackAnchorTotalLines: screen.totalLines }
        : {}),
    });

    // Fetch from the end of scrollback backwards
    const remaining = scrollbackAvailable - screen.scrollbackOffset;
    const limit = Math.min(SCROLLBACK_PAGE_SIZE, remaining);
    const offset = scrollbackAvailable - screen.scrollbackOffset - limit;

    client
      .getScrollback(session, Math.max(0, offset), limit)
      .then((resp) => {
        const sig = getScreenSignal(session);
        const current = sig.value;
        const currentAnchor = current.scrollbackAnchorTotalLines ?? resp.total_lines;
        const currentAvailable = Math.max(0, currentAnchor - current.rows);

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
  }, [client, session, screen.scrollbackComplete, screen.scrollbackLoading, screen.alternateActive, screen.totalLines, screen.rows, screen.scrollbackOffset, screen.scrollbackAnchorTotalLines]);

  // Track manual scrolling + trigger scrollback fetch near top
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const handleScroll = () => {
      const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 30;
      userScrolledRef.current = !atBottom;
      setScrollTop(el.scrollTop);

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
    // Touch-based equivalent of the wheel handler for mobile.  On mobile
    // there are no wheel events; touch scrolling only produces "scroll"
    // events when the element already overflows.  This detects an upward
    // scroll gesture (finger moves down) while at scrollTop === 0 and
    // triggers the initial scrollback load so the container gains overflow
    // and native touch scrolling takes over.
    let touchStartY = 0;
    const handleTouchStart = (e: TouchEvent) => {
      if (e.touches.length === 1) {
        touchStartY = e.touches[0].clientY;
      }
    };
    const handleTouchMove = (e: TouchEvent) => {
      if (e.touches.length === 1 && el.scrollTop === 0) {
        const dy = e.touches[0].clientY - touchStartY;
        if (dy > 10) {
          userScrolledRef.current = true;
          fetchScrollback();
        }
      }
    };
    el.addEventListener("scroll", handleScroll);
    el.addEventListener("wheel", handleWheel, { passive: true });
    el.addEventListener("touchstart", handleTouchStart, { passive: true });
    el.addEventListener("touchmove", handleTouchMove, { passive: true });
    return () => {
      el.removeEventListener("scroll", handleScroll);
      el.removeEventListener("wheel", handleWheel);
      el.removeEventListener("touchstart", handleTouchStart);
      el.removeEventListener("touchmove", handleTouchMove);
    };
  }, [fetchScrollback]);

  // Auto-scroll to bottom when new content arrives (only in normal mode, only if at bottom)
  useEffect(() => {
    if (screen.alternateActive || userScrolledRef.current) return;
    const el = containerRef.current;
    if (!el) return;
    requestAnimationFrame(() => {
      el.scrollTop = el.scrollHeight;
    });
  }, [screen.lines, screen.totalLines, screen.alternateActive]);

  const containerClass = screen.alternateActive
    ? "terminal-container alternate"
    : "terminal-container";

  // Cursor is relative to the screen lines (not scrollback)
  const cursorLineIndex = screen.cursor.visible
    ? scrollbackLen + screen.cursor.row
    : -1;

  // Extract overlays and panels from screen state
  const overlays = screen.overlays || [];
  const panels = screen.panels || [];

  // Compute panel layout based on current screen mode
  const activePanels = panels.filter(
    (p) => p.visible && (p.screen_mode ?? "normal") === (screen.alternateActive ? "alt" : "normal"),
  );
  const panelLayout = computePanelLayout(activePanels, screen.rows);

  // ---------------------------------------------------------------------------
  // Virtualized rendering — only create DOM elements for visible lines + overscan
  // In alternate screen mode, always render all lines (small fixed grid).
  // ---------------------------------------------------------------------------
  let lineElements: preact.JSX.Element | preact.JSX.Element[];
  if (screen.alternateActive) {
    // Alternate screen: render all lines directly (small fixed grid)
    lineElements = screen.lines.map((line, i) =>
      <MemoLine key={i} line={line} lineIdx={i} cursor={i === cursorLineIndex ? { col: screen.cursor.col } : null} />,
    );
  } else {
    // Normal mode: virtualized rendering
    const viewportHeight = containerRef.current?.clientHeight ?? 0;
    const rangeStart = Math.max(0, Math.floor(scrollTop / lineHeight) - OVERSCAN);
    const rangeEnd = Math.min(totalLines, Math.ceil((scrollTop + viewportHeight) / lineHeight) + OVERSCAN);

    const topPad = rangeStart * lineHeight;
    const bottomPad = (totalLines - rangeEnd) * lineHeight;

    const visibleLines: preact.JSX.Element[] = [];
    for (let i = rangeStart; i < rangeEnd; i++) {
      const line = getLine(screen.scrollbackLines, screen.lines, i);
      // Use stable keys: "sb-N" for scrollback, "sc-N" for screen lines.
      // This prevents Preact from re-creating all DOM nodes when scrollback
      // is prepended and indices shift.
      const key = i < scrollbackLen ? `sb-${i}` : `sc-${i - scrollbackLen}`;
      visibleLines.push(
        <MemoLine key={key} line={line} lineIdx={i} cursor={i === cursorLineIndex ? { col: screen.cursor.col } : null} />,
      );
    }

    lineElements = (
      <>
        {topPad > 0 && <div style={{ height: `${topPad}px` }} />}
        {visibleLines}
        {bottomPad > 0 && <div style={{ height: `${bottomPad}px` }} />}
      </>
    );
  }

  return (
    <div
      class="terminal-wrapper"
      style={{ fontSize: `${fontSize}px` }}
      onClick={handleWrapperClick}
    >
      {captureInput && (
        <textarea
          ref={textareaRef}
          class="terminal-hidden-input"
          onKeyDown={handleTextareaKeyDown}
          onInput={handleTextareaInput}
          autocomplete="off"
          autocapitalize="off"
          autocorrect="off"
          spellcheck={false}
          aria-label={`Terminal input for ${session}`}
        />
      )}
      {cellSize && panelLayout.topPanels.length > 0 && (
        <PanelRegion panels={panelLayout.topPanels} charWidth={cellSize.w} charHeight={cellSize.h} />
      )}
      <div
        class={containerClass}
        ref={containerRef}
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
        {lineElements}
        {cellSize && overlays.length > 0 && (
          <OverlayLayer overlays={overlays} charWidth={cellSize.w} charHeight={cellSize.h} />
        )}
        {disconnected && (
          <div class="terminal-disconnected">Connection lost</div>
        )}
      </div>
      {cellSize && panelLayout.bottomPanels.length > 0 && (
        <PanelRegion panels={panelLayout.bottomPanels} charWidth={cellSize.w} charHeight={cellSize.h} />
      )}
    </div>
  );
}
