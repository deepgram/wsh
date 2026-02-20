import type { Span, Color } from "../api/types";

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

export function colorToCSS(c: Color): string {
  init256();
  if (c.rgb) return `rgb(${c.rgb.r},${c.rgb.g},${c.rgb.b})`;
  if (c.indexed !== undefined) {
    const i = c.indexed;
    if (i < 16) return `var(--c${i})`;
    return ANSI_256[i] ?? "inherit";
  }
  return "inherit";
}

export function spanStyle(span: Span): Record<string, string> {
  const s: Record<string, string> = {};

  const fg = span.fg ? colorToCSS(span.fg) : null;
  const bg = span.bg ? colorToCSS(span.bg) : null;

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
