import { useRef, useEffect } from "preact/hooks";
import { getScreen } from "../state/terminal";
import { screens } from "../state/terminal";
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
  if (span.fg) s.color = colorToCSS(span.fg);
  if (span.bg) s.backgroundColor = colorToCSS(span.bg);
  if (span.bold) s.fontWeight = "bold";
  if (span.faint) s.opacity = "0.5";
  if (span.italic) s.fontStyle = "italic";

  const decorations: string[] = [];
  if (span.underline) decorations.push("underline");
  if (span.strikethrough) decorations.push("line-through");
  if (decorations.length > 0) s.textDecoration = decorations.join(" ");

  return s;
}

function renderLine(line: FormattedLine, lineIdx: number): preact.JSX.Element {
  if (typeof line === "string") {
    return (
      <div class="term-line" key={lineIdx}>
        {line || "\u00A0"}
      </div>
    );
  }

  // Styled spans
  if (line.length === 0) {
    return (
      <div class="term-line" key={lineIdx}>
        {"\u00A0"}
      </div>
    );
  }

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

interface TerminalProps {
  session: string;
}

export function Terminal({ session }: TerminalProps) {
  const containerRef = useRef<HTMLDivElement>(null);

  // Subscribe to screens signal to trigger re-renders
  const _screens = screens.value;

  // Auto-scroll to bottom when new content arrives (only in normal mode)
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const screen = getScreen(session);
    if (!screen.alternateActive) {
      el.scrollTop = el.scrollHeight;
    }
  });

  const screen = getScreen(session);
  const containerClass = screen.alternateActive
    ? "terminal-container alternate"
    : "terminal-container";

  return (
    <div class={containerClass} ref={containerRef}>
      {screen.lines.map((line, i) => renderLine(line, i))}
    </div>
  );
}
