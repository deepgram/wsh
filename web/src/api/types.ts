// Types matching the Rust API (src/parser/state.rs, src/parser/events.rs)

export interface Color {
  indexed?: number;
  rgb?: { r: number; g: number; b: number };
}

export interface Style {
  fg?: Color;
  bg?: Color;
  bold?: boolean;
  faint?: boolean;
  italic?: boolean;
  underline?: boolean;
  strikethrough?: boolean;
  blink?: boolean;
  inverse?: boolean;
}

export interface Span {
  text: string;
  fg?: Color;
  bg?: Color;
  bold?: boolean;
  faint?: boolean;
  italic?: boolean;
  underline?: boolean;
  strikethrough?: boolean;
  blink?: boolean;
  inverse?: boolean;
}

// FormattedLine is either a plain string or an array of Spans (serde untagged)
export type FormattedLine = string | Span[];

export interface Cursor {
  row: number;
  col: number;
  visible: boolean;
}

export interface ScreenResponse {
  epoch: number;
  first_line_index: number;
  total_lines: number;
  lines: FormattedLine[];
  cursor: Cursor;
  cols: number;
  rows: number;
  alternate_active: boolean;
}

export interface ScrollbackResponse {
  epoch: number;
  lines: FormattedLine[];
  total_lines: number;
  offset: number;
}

// Events from the server (src/parser/events.rs)

export type Event =
  | { event: "line"; seq: number; index: number; total_lines: number; line: FormattedLine }
  | { event: "cursor"; seq: number; row: number; col: number; visible: boolean }
  | { event: "mode"; seq: number; alternate_active: boolean }
  | { event: "reset"; seq: number; reason: string }
  | { event: "sync"; seq: number; screen: ScreenResponse; scrollback_lines: number }
  | { event: "diff"; seq: number; changed_lines: number[]; screen: ScreenResponse }
  | { event: "idle"; seq: number; generation: number; screen: ScreenResponse; scrollback_lines: number }
  | { event: "running"; seq: number; generation: number };

// WebSocket request/response envelopes

export interface WsRequest {
  id?: number;
  method: string;
  session?: string;
  params?: unknown;
}

export interface WsResponse {
  id?: number;
  method?: string;
  result?: unknown;
  error?: { code: string; message: string };
}

export type EventType = "lines" | "cursor" | "mode" | "diffs" | "activity" | "overlay";

export interface SessionInfo {
  name: string;
  pid: number | null;
  command: string;
  rows: number;
  cols: number;
  clients: number;
  tags: string[];
  server?: string;
}

export interface ServerInfo {
  hostname: string;
  address: string;
  health: "healthy" | "connecting" | "unhealthy" | string;
  role: string;
  sessions?: number;
}

export interface SendInputResult {
  generation: number;
}

// Overlay color — different from terminal Color.
// Named colors serialize as bare strings (e.g. "red"), RGB as {r, g, b}.
// Serde #[serde(untagged)] on the Rust enum produces this shape.
export type OverlayColor = string | { r: number; g: number; b: number };

export interface OverlaySpan {
  text: string;
  id?: string;
  fg?: OverlayColor;
  bg?: OverlayColor;
  bold?: boolean;
  italic?: boolean;
  underline?: boolean;
}

export interface RegionWrite {
  row: number;
  col: number;
  text: string;
  fg?: OverlayColor;
  bg?: OverlayColor;
  bold?: boolean;
  italic?: boolean;
  underline?: boolean;
}

export interface BackgroundStyle {
  bg: OverlayColor;
}

export interface Overlay {
  id: string;
  x: number;
  y: number;
  z: number;
  width: number;
  height: number;
  background?: BackgroundStyle;
  spans: OverlaySpan[];
  region_writes: RegionWrite[];
  focusable?: boolean;
  screen_mode?: "normal" | "alt";
}

export interface Panel {
  id: string;
  position: "top" | "bottom";
  height: number;
  z: number;
  background?: BackgroundStyle;
  spans: OverlaySpan[];
  region_writes: RegionWrite[];
  visible: boolean;
  focusable?: boolean;
  screen_mode?: "normal" | "alt";
}
