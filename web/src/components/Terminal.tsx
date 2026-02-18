import { useRef, useEffect } from "preact/hooks";
import { getScreenSignal } from "../state/terminal";
import { connectionState } from "../state/sessions";
import type { FormattedLine, Span, Color } from "../api/types";

// ANSI 256-color palette (first 16 use CSS vars, rest computed)
const ANSI_256: string[] = [];

function init256(): void {
  if (ANSI_256.length > 0) return;
  // 0-15: use CSS custom properties (handled separately)
  for (let i = 0; i < 16; i++) ANSI_256.push("");
  // 16-231: 6x6x6 color cube
  for (let r = 0; r < 6; r++) {
    for (let g = 0; g < 6; g++) {
      for (let b = 0; b < 6; b++) {
        ANSI_256.push(
          `rgb(${r ? r * 40 + 55 : 0},${g ? g * 40 + 55 : 0},${b ? b * 40 + 55 : 0})`,
        );
      }
    }
  }
  // 232-255: grayscale
  for (let i = 0; i < 24; i++) {
    const v = i * 10 + 8;
    ANSI_256.push(`rgb(${v},${v},${v})`);
  }
}

function colorToCSS(c: Color): string {
  init256();
  if (c.rgb) return `rgb(${c.rgb.r},${c.rgb.g},${c.rgb.b})`;
  if (c.indexed !== undefined) {
    const i = c.indexed;
    if (i < 16) return `var(--c${i})`;
    return ANSI_256[i] ?? "inherit";
  }
  return "inherit";
}

function spanStyle(span: Span): Record<string, string> {
  const s: Record<string, string> = {};

  let fg = span.fg ? colorToCSS(span.fg) : null;
  let bg = span.bg ? colorToCSS(span.bg) : null;

  if (span.inverse) {
    s.color = bg ?? "var(--bg)";
    s.backgroundColor = fg ?? "var(--fg)";
  } else {
    if (fg) s.color = fg;
    if (bg) s.backgroundColor = bg;
  }

  if (span.bold) s.fontWeight = "bold";
  if (span.faint) s.opacity = "0.5";
  if (span.italic) s.fontStyle = "italic";

  const decorations: string[] = [];
  if (span.underline) decorations.push("underline");
  if (span.strikethrough) decorations.push("line-through");
  if (decorations.length > 0) s.textDecoration = decorations.join(" ");

  return s;
}

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

interface TerminalProps {
  session: string;
  client?: import("../api/ws").WshClient;
}

function measureChar(container: HTMLElement): { w: number; h: number } {
  const span = document.createElement("span");
  span.className = "term-line";
  span.textContent = "X";
  span.style.position = "absolute";
  span.style.visibility = "hidden";
  container.appendChild(span);
  const rect = span.getBoundingClientRect();
  container.removeChild(span);
  return { w: rect.width, h: rect.height };
}

export function Terminal({ session, client }: TerminalProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const userScrolledRef = useRef(false);

  // Subscribe only to this session's signal (not all sessions)
  const screen = getScreenSignal(session).value;
  const disconnected = connectionState.value !== "connected";

  // Resize PTY to match container dimensions
  useEffect(() => {
    const el = containerRef.current;
    if (!el || !client) return;

    let prevCols = 0;
    let prevRows = 0;

    const observer = new ResizeObserver(() => {
      const { w, h } = measureChar(el);
      if (w === 0 || h === 0) return;
      const cols = Math.floor(el.clientWidth / w);
      const rows = Math.floor(el.clientHeight / h);
      if (cols < 1 || rows < 1) return;
      if (cols === prevCols && rows === prevRows) return;
      prevCols = cols;
      prevRows = rows;
      client.resize(session, cols, rows).catch((e) => {
        console.error(`Failed to resize session "${session}":`, e);
      });
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, [session, client]);

  // Track manual scrolling
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const handleScroll = () => {
      const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 30;
      userScrolledRef.current = !atBottom;
    };
    el.addEventListener("scroll", handleScroll);
    return () => el.removeEventListener("scroll", handleScroll);
  }, []);

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

  const cursorLineIndex = screen.cursor.visible
    ? screen.firstLineIndex + screen.cursor.row
    : -1;

  return (
    <div class={containerClass} ref={containerRef}>
      {screen.lines.map((line, i) =>
        renderLine(line, i, i === cursorLineIndex ? { col: screen.cursor.col } : null),
      )}
      {disconnected && (
        <div class="terminal-disconnected">Connection lost</div>
      )}
    </div>
  );
}
