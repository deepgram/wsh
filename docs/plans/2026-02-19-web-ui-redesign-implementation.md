# Web UI Redesign Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Rebuild the web UI layout around a sidebar + main content model with tag-based session grouping, three view modes (depth carousel, auto-grid tiles, quiescence queue), drag-and-drop tag management, command palette, and 6 polished themes.

**Architecture:** Hybrid rebuild. Keep `Terminal.tsx`, `InputBar.tsx`, `ErrorBoundary.tsx`, `WshClient`, `api/types.ts`, and `state/terminal.ts` intact. Rebuild the layout shell, sidebar, view modes, and navigation. All state is Preact Signals. No new backend work — the existing WS JSON-RPC API (including tags, quiescence, lifecycle events) covers everything.

**Tech Stack:** Preact 10 + Preact Signals + TypeScript + Vite 6. No new dependencies.

---

## Phase 1: State Foundation & WshClient Extensions

### Task 1: Extend WshClient with Tag and Session Info Methods

**Files:**
- Modify: `web/src/api/ws.ts`
- Modify: `web/src/api/types.ts`

**Step 1: Add types for enriched session info**

In `web/src/api/types.ts`, add after the `EventType` type:

```typescript
export interface SessionInfo {
  name: string;
  pid: number | null;
  command: string;
  rows: number;
  cols: number;
  clients: number;
  tags: string[];
}
```

**Step 2: Update WshClient convenience methods**

In `web/src/api/ws.ts`, update `listSessions` and `createSession` to return `SessionInfo`, and add tag methods:

```typescript
async listSessions(tags?: string[]): Promise<SessionInfo[]> {
  const params = tags && tags.length > 0 ? { tag: tags } : undefined;
  const result = await this.request("list_sessions", params);
  return result as SessionInfo[];
}

async createSession(name?: string, tags?: string[]): Promise<SessionInfo> {
  const params: Record<string, unknown> = {};
  if (name) params.name = name;
  if (tags && tags.length > 0) params.tags = tags;
  const result = await this.request("create_session", params);
  return result as SessionInfo;
}

async updateSession(name: string, updates: {
  name?: string;
  add_tags?: string[];
  remove_tags?: string[];
}): Promise<SessionInfo> {
  const result = await this.request("update_session", updates, name);
  return result as SessionInfo;
}

async awaitQuiesce(session: string, timeout?: number, tags?: string[]): Promise<{ session: string }> {
  const params: Record<string, unknown> = {};
  if (timeout !== undefined) params.max_wait = timeout;
  if (tags && tags.length > 0) params.tags = tags;
  const result = await this.request("await_quiesce", params, session);
  return result as { session: string };
}
```

Also update the import in types.ts to export `SessionInfo`.

**Step 3: Run the build to verify types compile**

Run: `cd web && bun run build`
Expected: Build succeeds with no type errors.

**Step 4: Commit**

```bash
git add web/src/api/ws.ts web/src/api/types.ts
git commit -m "feat(web): extend WshClient with tag and session info methods"
```

### Task 2: New State Module for Groups, Sidebar, and Queue

**Files:**
- Create: `web/src/state/groups.ts`
- Modify: `web/src/state/sessions.ts`

**Step 1: Extend sessions state with tag and session info tracking**

In `web/src/state/sessions.ts`, add new signals and update the Theme type:

```typescript
export type Theme = "glass" | "neon" | "minimal" | "tokyo-night" | "catppuccin" | "dracula";
export type ViewMode = "carousel" | "tiled" | "queue";

// Replace the old viewMode signal
export const viewMode = signal<ViewMode>("carousel");

// New signals
export const sidebarWidth = signal<number>(
  parseFloat(localStorage.getItem("wsh-sidebar-width") || "15")
);
export const sidebarCollapsed = signal<boolean>(
  localStorage.getItem("wsh-sidebar-collapsed") === "true"
);

// Per-session info cache (tags, pid, command, etc.)
export const sessionInfoMap = signal<Map<string, SessionInfo>>(new Map());
```

Remove `tileSelection`, `toggleTileSelection`, `clearTileSelection` (these move to the new tile logic). Replace `cycleTheme` with `setTheme`:

```typescript
export function setTheme(t: Theme): void {
  theme.value = t;
  localStorage.setItem("wsh-theme", t);
}
```

Update `storedTheme` validation to include new theme names.

**Step 2: Create groups state module**

Create `web/src/state/groups.ts`:

```typescript
import { computed, signal } from "@preact/signals";
import { sessionInfoMap, type ViewMode } from "./sessions";
import type { SessionInfo } from "../api/types";

export interface Group {
  tag: string;           // "all" for meta-group, "untagged" for untagged
  label: string;         // Display name
  sessions: string[];    // Session names in this group
  isSpecial: boolean;    // true for "all" and "untagged"
  badgeCount: number;    // Sessions needing attention (quiescent + exited)
}

export interface QueueEntry {
  session: string;
  quiescentAt: number;   // timestamp
  status: "pending" | "handled";
}

// Which groups are selected in the sidebar
export const selectedGroups = signal<string[]>(["all"]);

// View mode remembered per group tag
const storedViewModes = JSON.parse(localStorage.getItem("wsh-view-modes") || "{}");
export const viewModePerGroup = signal<Record<string, ViewMode>>(storedViewModes);

// Quiescence queue per group tag
export const quiescenceQueues = signal<Record<string, QueueEntry[]>>({});

// Session status tracking (derived from lifecycle events + quiescence)
export type SessionStatus = "running" | "quiescent" | "exited";
export const sessionStatuses = signal<Map<string, SessionStatus>>(new Map());

// Tile layout per group (session order + relative sizes)
export const tileLayouts = signal<Record<string, {
  sessions: string[];
  sizes: number[];
}>>(JSON.parse(localStorage.getItem("wsh-tile-layouts") || "{}"));

// Computed groups derived from sessionInfoMap
export const groups = computed<Group[]>(() => {
  const infoMap = sessionInfoMap.value;
  const statuses = sessionStatuses.value;
  const tagGroups = new Map<string, string[]>();
  const untagged: string[] = [];

  for (const [name, info] of infoMap) {
    if (info.tags.length === 0) {
      untagged.push(name);
    } else {
      for (const tag of info.tags) {
        const group = tagGroups.get(tag) || [];
        group.push(name);
        tagGroups.set(tag, group);
      }
    }
  }

  const result: Group[] = [];

  // "All Sessions" always first
  const allSessions = Array.from(infoMap.keys());
  const allBadge = allSessions.filter(
    (s) => statuses.get(s) === "quiescent" || statuses.get(s) === "exited"
  ).length;
  result.push({
    tag: "all",
    label: "All Sessions",
    sessions: allSessions,
    isSpecial: true,
    badgeCount: allBadge,
  });

  // Tag groups alphabetically
  const sortedTags = Array.from(tagGroups.keys()).sort();
  for (const tag of sortedTags) {
    const sessions = tagGroups.get(tag)!;
    const badge = sessions.filter(
      (s) => statuses.get(s) === "quiescent" || statuses.get(s) === "exited"
    ).length;
    result.push({
      tag,
      label: tag,
      sessions,
      isSpecial: false,
      badgeCount: badge,
    });
  }

  // "Untagged" always last (only if there are untagged sessions)
  if (untagged.length > 0) {
    const badge = untagged.filter(
      (s) => statuses.get(s) === "quiescent" || statuses.get(s) === "exited"
    ).length;
    result.push({
      tag: "untagged",
      label: "Untagged",
      sessions: untagged,
      isSpecial: true,
      badgeCount: badge,
    });
  }

  return result;
});

// Helper: get sessions for currently selected groups
export const activeGroupSessions = computed<string[]>(() => {
  const selected = selectedGroups.value;
  const allGroups = groups.value;
  const sessionSet = new Set<string>();
  for (const tag of selected) {
    const group = allGroups.find((g) => g.tag === tag);
    if (group) {
      for (const s of group.sessions) sessionSet.add(s);
    }
  }
  return Array.from(sessionSet);
});

// Helper: get current view mode for selected group
export function getViewModeForGroup(tag: string): ViewMode {
  return viewModePerGroup.value[tag] || "carousel";
}

export function setViewModeForGroup(tag: string, mode: ViewMode): void {
  const updated = { ...viewModePerGroup.value, [tag]: mode };
  viewModePerGroup.value = updated;
  localStorage.setItem("wsh-view-modes", JSON.stringify(updated));
}

// Queue management
export function enqueueSession(tag: string, session: string): void {
  const queues = { ...quiescenceQueues.value };
  const queue = [...(queues[tag] || [])];
  // Don't add if already in pending queue
  if (queue.some((e) => e.session === session && e.status === "pending")) return;
  queue.push({ session, quiescentAt: Date.now(), status: "pending" });
  queues[tag] = queue;
  quiescenceQueues.value = queues;
}

export function dismissQueueEntry(tag: string, session: string): void {
  const queues = { ...quiescenceQueues.value };
  const queue = (queues[tag] || []).map((e) =>
    e.session === session ? { ...e, status: "handled" as const } : e
  );
  queues[tag] = queue;
  quiescenceQueues.value = queues;
}
```

**Step 3: Run the build to verify types compile**

Run: `cd web && bun run build`
Expected: Build succeeds (groups module not yet imported by app.tsx, but should compile standalone).

**Step 4: Commit**

```bash
git add web/src/state/groups.ts web/src/state/sessions.ts
git commit -m "feat(web): add groups state module and extend session state for sidebar"
```

---

## Phase 2: Layout Shell

### Task 3: Create the Layout Shell Component

**Files:**
- Create: `web/src/components/LayoutShell.tsx`
- Modify: `web/src/app.tsx`

**Step 1: Build the layout shell**

Create `web/src/components/LayoutShell.tsx`. This is the root layout: sidebar (left) + main content (right). The sidebar is resizable via a drag handle.

```typescript
import { useRef, useCallback, useEffect } from "preact/hooks";
import { sidebarWidth, sidebarCollapsed } from "../state/sessions";
import type { WshClient } from "../api/ws";
import { Sidebar } from "./Sidebar";
import { MainContent } from "./MainContent";

interface LayoutShellProps {
  client: WshClient;
}

export function LayoutShell({ client }: LayoutShellProps) {
  const dragging = useRef(false);
  const containerRef = useRef<HTMLDivElement>(null);

  const collapsed = sidebarCollapsed.value;
  const width = sidebarWidth.value;

  const handleMouseDown = useCallback((e: MouseEvent) => {
    e.preventDefault();
    dragging.current = true;
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
  }, []);

  useEffect(() => {
    const handleMouseMove = (e: MouseEvent) => {
      if (!dragging.current || !containerRef.current) return;
      const rect = containerRef.current.getBoundingClientRect();
      const pct = ((e.clientX - rect.left) / rect.width) * 100;
      const clamped = Math.max(10, Math.min(30, pct));
      sidebarWidth.value = clamped;
      localStorage.setItem("wsh-sidebar-width", String(clamped));
    };
    const handleMouseUp = () => {
      if (dragging.current) {
        dragging.current = false;
        document.body.style.cursor = "";
        document.body.style.userSelect = "";
      }
    };
    window.addEventListener("mousemove", handleMouseMove);
    window.addEventListener("mouseup", handleMouseUp);
    return () => {
      window.removeEventListener("mousemove", handleMouseMove);
      window.removeEventListener("mouseup", handleMouseUp);
    };
  }, []);

  const toggleCollapse = useCallback(() => {
    sidebarCollapsed.value = !sidebarCollapsed.value;
    localStorage.setItem("wsh-sidebar-collapsed", String(sidebarCollapsed.value));
  }, []);

  return (
    <div class="layout-shell" ref={containerRef}>
      <div
        class={`layout-sidebar ${collapsed ? "collapsed" : ""}`}
        style={collapsed ? undefined : { width: `${width}%` }}
      >
        <Sidebar client={client} collapsed={collapsed} onToggleCollapse={toggleCollapse} />
      </div>
      {!collapsed && (
        <div class="layout-resize-handle" onMouseDown={handleMouseDown} />
      )}
      <div class="layout-main">
        <MainContent client={client} />
      </div>
    </div>
  );
}
```

**Step 2: Create stub Sidebar and MainContent components**

Create `web/src/components/Sidebar.tsx` (stub):

```typescript
import type { WshClient } from "../api/ws";
import { groups, selectedGroups } from "../state/groups";

interface SidebarProps {
  client: WshClient;
  collapsed: boolean;
  onToggleCollapse: () => void;
}

export function Sidebar({ client, collapsed, onToggleCollapse }: SidebarProps) {
  const allGroups = groups.value;
  const selected = selectedGroups.value;

  if (collapsed) {
    return (
      <div class="sidebar-collapsed">
        <button class="sidebar-expand-btn" onClick={onToggleCollapse} title="Expand sidebar">
          &#9656;
        </button>
        {allGroups.map((g) => (
          <div
            key={g.tag}
            class={`sidebar-icon ${selected.includes(g.tag) ? "active" : ""}`}
            onClick={() => { selectedGroups.value = [g.tag]; }}
            title={g.label}
          >
            {g.badgeCount > 0 && <span class="sidebar-badge">{g.badgeCount}</span>}
          </div>
        ))}
      </div>
    );
  }

  return (
    <div class="sidebar-content">
      <div class="sidebar-header">
        <span class="sidebar-title">Sessions</span>
        <button class="sidebar-collapse-btn" onClick={onToggleCollapse} title="Collapse sidebar">
          &#9666;
        </button>
      </div>
      <div class="sidebar-groups">
        {allGroups.map((g) => (
          <div
            key={g.tag}
            class={`sidebar-group ${selected.includes(g.tag) ? "selected" : ""}`}
            onClick={() => { selectedGroups.value = [g.tag]; }}
          >
            <div class="sidebar-group-header">
              <span class="sidebar-group-label">{g.label}</span>
              <span class="sidebar-group-count">{g.sessions.length}</span>
              {g.badgeCount > 0 && <span class="sidebar-badge">{g.badgeCount}</span>}
            </div>
            {/* Live mini-previews will go here in Task 5 */}
          </div>
        ))}
      </div>
      <div class="sidebar-footer">
        {/* Connection status, theme picker, new session — Task 6 */}
      </div>
    </div>
  );
}
```

Create `web/src/components/MainContent.tsx` (stub):

```typescript
import type { WshClient } from "../api/ws";
import { selectedGroups, getViewModeForGroup } from "../state/groups";
import { activeGroupSessions } from "../state/groups";

interface MainContentProps {
  client: WshClient;
}

export function MainContent({ client }: MainContentProps) {
  const selected = selectedGroups.value;
  const primaryTag = selected[0] || "all";
  const mode = getViewModeForGroup(primaryTag);
  const sessions = activeGroupSessions.value;

  return (
    <div class="main-content">
      <div class="main-header">
        <span class="main-group-name">{primaryTag}</span>
        <div class="view-mode-toggle">
          {/* Carousel / Tiled / Queue toggle buttons — Task 7 */}
        </div>
      </div>
      <div class="main-body">
        <div class="main-placeholder">
          {sessions.length} sessions in {mode} mode
        </div>
      </div>
    </div>
  );
}
```

**Step 3: Wire LayoutShell into app.tsx**

Replace the current view mode rendering in `app.tsx`:

```typescript
// Replace imports of SessionCarousel, SessionGrid, TiledLayout, StatusBar
import { LayoutShell } from "./components/LayoutShell";

// In the App return:
return (
  <ErrorBoundary>
    <LayoutShell client={client} />
  </ErrorBoundary>
);
```

Update `initSessions` to populate `sessionInfoMap` with tag data from `listSessions`.

**Step 4: Add layout CSS**

In `web/src/styles/terminal.css`, add layout shell styles:

```css
.layout-shell {
  display: flex;
  height: 100vh;
  width: 100vw;
  overflow: hidden;
}

.layout-sidebar {
  flex-shrink: 0;
  height: 100%;
  overflow: hidden;
  transition: width 0.2s ease;
}

.layout-sidebar.collapsed {
  width: 40px;
}

.layout-resize-handle {
  width: 4px;
  cursor: col-resize;
  background: transparent;
  flex-shrink: 0;
  transition: background 0.15s;
}

.layout-resize-handle:hover {
  background: var(--accent, #666);
}

.layout-main {
  flex: 1;
  min-width: 0;
  height: 100%;
  display: flex;
  flex-direction: column;
}
```

**Step 5: Run the build**

Run: `cd web && bun run build`
Expected: Build succeeds. The app renders with a sidebar + main area.

**Step 6: Commit**

```bash
git add web/src/components/LayoutShell.tsx web/src/components/Sidebar.tsx web/src/components/MainContent.tsx web/src/app.tsx web/src/styles/terminal.css
git commit -m "feat(web): add layout shell with sidebar and main content area"
```

### Task 4: Update app.tsx Session Initialization for Tags

**Files:**
- Modify: `web/src/app.tsx`

**Step 1: Update initSessions to populate sessionInfoMap**

The existing `initSessions` calls `client.listSessions()` which now returns `SessionInfo[]` with tags. Populate `sessionInfoMap`:

```typescript
import { sessionInfoMap } from "./state/sessions";

async function initSessions(client: WshClient): Promise<void> {
  // ... existing cleanup code ...
  const list = await client.listSessions();
  let infos = list;

  if (infos.length === 0) {
    const created = await client.createSession();
    infos = [created];
  }

  const names = infos.map((s) => s.name);
  sessions.value = names;
  sessionOrder.value = [...names];

  // Populate session info map with tag data
  const infoMap = new Map<string, SessionInfo>();
  for (const info of infos) {
    infoMap.set(info.name, info);
  }
  sessionInfoMap.value = infoMap;

  // ... rest of existing init code ...
}
```

**Step 2: Update lifecycle event handlers to maintain sessionInfoMap**

In `handleLifecycleEvent`, update the info map on create/destroy/rename events. On `session_created`, fetch session info and add to map. On `session_destroyed`, remove from map.

**Step 3: Run the build**

Run: `cd web && bun run build`
Expected: Build succeeds.

**Step 4: Commit**

```bash
git add web/src/app.tsx
git commit -m "feat(web): populate sessionInfoMap with tag data on init"
```

---

## Phase 3: Sidebar Full Implementation

### Task 5: Sidebar with Live Mini-Previews

**Files:**
- Modify: `web/src/components/Sidebar.tsx`
- Create: `web/src/components/MiniTerminal.tsx`

**Step 1: Create MiniTerminal component**

A tiny, non-interactive terminal renderer that shows a scaled-down live preview. It reads from the same per-session screen signal that `Terminal.tsx` uses, but renders at a much smaller scale with no scrollback or input.

```typescript
import { getScreenSignal } from "../state/terminal";
import type { FormattedLine, Span, Color } from "../api/types";

interface MiniTerminalProps {
  session: string;
}

// Reuse colorToCSS and spanStyle from Terminal.tsx (extract to shared util in a later task)
// For now, render plain text only — styled version comes in polish phase

export function MiniTerminal({ session }: MiniTerminalProps) {
  const screen = getScreenSignal(session).value;

  return (
    <div class="mini-terminal">
      {screen.lines.slice(0, 8).map((line, i) => (
        <div key={i} class="mini-term-line">
          {typeof line === "string" ? line : line.map((s) => s.text).join("")}
        </div>
      ))}
    </div>
  );
}
```

**Step 2: Integrate MiniTerminal into Sidebar groups**

Update `Sidebar.tsx` to render a grid of `MiniTerminal` previews for each group's sessions (show up to 4 in a 2x2 mini-grid).

**Step 3: Add sidebar group multi-select**

Handle Ctrl+click and Shift+click on sidebar groups:

```typescript
const handleGroupClick = (tag: string, e: MouseEvent) => {
  if (e.ctrlKey || e.metaKey) {
    // Toggle selection
    const current = selectedGroups.value;
    if (current.includes(tag)) {
      selectedGroups.value = current.filter((t) => t !== tag);
    } else {
      selectedGroups.value = [...current, tag];
    }
  } else {
    selectedGroups.value = [tag];
  }
};
```

**Step 4: Add status dots to mini-previews**

Use `sessionStatuses` signal to render colored dots on each mini-session.

**Step 5: Add timestamps**

Show "Last active Xm ago" below each group's preview. Compute from `sessionStatuses` and a periodic timer.

**Step 6: Style the sidebar**

Add CSS for `.sidebar-content`, `.sidebar-group`, `.mini-terminal`, `.mini-term-line`, `.sidebar-badge`, status dots, etc.

**Step 7: Run the build and test**

Run: `cd web && bun run build`
Expected: Build succeeds. Sidebar shows groups with live mini-previews.

**Step 8: Commit**

```bash
git add web/src/components/Sidebar.tsx web/src/components/MiniTerminal.tsx web/src/styles/terminal.css
git commit -m "feat(web): sidebar with live mini-previews, status dots, and multi-select"
```

### Task 6: Sidebar Footer (Connection, Theme Picker, New Session)

**Files:**
- Modify: `web/src/components/Sidebar.tsx`
- Create: `web/src/components/ThemePicker.tsx`

**Step 1: Create ThemePicker component**

A context menu dropdown that shows all 6 themes with color swatch previews:

```typescript
import { useState, useRef, useEffect } from "preact/hooks";
import { theme, setTheme, type Theme } from "../state/sessions";

const THEMES: { id: Theme; label: string; swatches: string[] }[] = [
  { id: "glass", label: "Glass", swatches: ["#1a1a2e", "#e0e0e8", "#6c63ff", "#44d7b6", "#888"] },
  { id: "neon", label: "Neon", swatches: ["#05050a", "#00ffcc", "#ff2d95", "#00aaff", "#ffdd00"] },
  { id: "minimal", label: "Minimal", swatches: ["#161618", "#c8c8cc", "#f5f5f7", "#81c784", "#64b5f6"] },
  { id: "tokyo-night", label: "Tokyo Night", swatches: ["#1a1b26", "#a9b1d6", "#7aa2f7", "#9ece6a", "#f7768e"] },
  { id: "catppuccin", label: "Catppuccin", swatches: ["#1e1e2e", "#cdd6f4", "#cba6f7", "#a6e3a1", "#f38ba8"] },
  { id: "dracula", label: "Dracula", swatches: ["#282a36", "#f8f8f2", "#bd93f9", "#50fa7b", "#ff79c6"] },
];

export function ThemePicker() {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const current = theme.value;

  // Close on click outside
  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [open]);

  return (
    <div class="theme-picker" ref={ref}>
      <button class="theme-picker-btn" onClick={() => setOpen(!open)} title="Change theme">
        &#9673;
      </button>
      {open && (
        <div class="theme-picker-menu">
          {THEMES.map((t) => (
            <button
              key={t.id}
              class={`theme-picker-option ${current === t.id ? "active" : ""}`}
              onClick={() => { setTheme(t.id); setOpen(false); }}
            >
              <div class="theme-swatches">
                {t.swatches.map((color, i) => (
                  <span key={i} class="theme-swatch" style={{ background: color }} />
                ))}
              </div>
              <span class="theme-name">{t.label}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
```

**Step 2: Build sidebar footer**

Add to `Sidebar.tsx` footer section: connection status dot, ThemePicker, keyboard help (?), and new session (+) button.

**Step 3: Style the footer and theme picker**

**Step 4: Run the build**

Run: `cd web && bun run build`
Expected: Build succeeds. Theme picker dropdown works.

**Step 5: Commit**

```bash
git add web/src/components/ThemePicker.tsx web/src/components/Sidebar.tsx web/src/styles/terminal.css
git commit -m "feat(web): sidebar footer with theme picker, connection status, new session button"
```

### Task 7: Tag Editing UX

**Files:**
- Create: `web/src/components/TagEditor.tsx`
- Modify: `web/src/components/Sidebar.tsx`

**Step 1: Create TagEditor popover component**

An inline popover with: editable text field with autocomplete from existing tags, "x" to remove current tag, "+" to add a new tag. Appears when clicking a tag name on a group or session.

**Step 2: Wire tag editing to WshClient.updateSession**

On tag change, call `client.updateSession(sessionName, { add_tags: [newTag], remove_tags: [oldTag] })`.

**Step 3: Style the popover**

**Step 4: Run the build and test**

Run: `cd web && bun run build`
Expected: Build succeeds.

**Step 5: Commit**

```bash
git add web/src/components/TagEditor.tsx web/src/components/Sidebar.tsx web/src/styles/terminal.css
git commit -m "feat(web): inline tag editing with autocomplete popover"
```

---

## Phase 4: View Modes

### Task 8: Depth Carousel

**Files:**
- Create: `web/src/components/DepthCarousel.tsx`
- Modify: `web/src/components/MainContent.tsx`

**Step 1: Build the DepthCarousel component**

The center session is ~70% width, fully interactive. Adjacent sessions are rendered at ~60% scale with CSS perspective transforms and reduced opacity. Smooth transitions via CSS transitions.

```typescript
interface DepthCarouselProps {
  sessions: string[];
  client: WshClient;
}
```

Key CSS: use `transform: perspective(1000px) translateX(...) translateZ(...) rotateY(...)` for the 3D effect. Center session gets `z-index: 2`, adjacent sessions get `z-index: 1` with `opacity: 0.6`.

**Step 2: Wire keyboard navigation**

Listen for Super+Left/Right (or Ctrl+Shift+Left/Right fallback) to rotate. Clicking a side preview snaps to center.

**Step 3: Mobile adaptation**

On viewports < 640px, render full-width single session with no side previews. Detect via `window.matchMedia`.

**Step 4: Wire into MainContent**

**Step 5: Style**

**Step 6: Run the build**

Run: `cd web && bun run build`
Expected: Build succeeds.

**Step 7: Commit**

```bash
git add web/src/components/DepthCarousel.tsx web/src/components/MainContent.tsx web/src/styles/terminal.css
git commit -m "feat(web): 3D depth carousel with keyboard navigation"
```

### Task 9: Auto-Grid Tiled View

**Files:**
- Create: `web/src/components/AutoGrid.tsx`
- Modify: `web/src/components/MainContent.tsx`

**Step 1: Build the auto-grid layout algorithm**

Given N sessions, compute the best NxM grid that minimizes wasted space. For 3: 2 top + 1 full-width bottom. For 5: 3 top + 2 wider bottom. For 7: 4 top + 3 bottom.

```typescript
function computeGridLayout(count: number): { rows: { count: number }[] } {
  if (count <= 0) return { rows: [] };
  if (count === 1) return { rows: [{ count: 1 }] };

  const cols = Math.ceil(Math.sqrt(count));
  const fullRows = Math.floor(count / cols);
  const remainder = count % cols;

  const rows: { count: number }[] = [];
  for (let i = 0; i < fullRows; i++) {
    rows.push({ count: cols });
  }
  if (remainder > 0) {
    rows.push({ count: remainder });
  }
  return { rows };
}
```

**Step 2: Build AutoGrid component**

Renders a CSS grid with cells sized proportionally. Each cell contains a `SessionPane` (Terminal + InputBar). Click to focus, double-click to switch to carousel mode for that session.

**Step 3: Drag-to-swap**

Implement HTML5 drag-and-drop within the grid. On drop, swap the two sessions' positions. Visual feedback: drop target highlights, dragged tile becomes semi-transparent.

**Step 4: Resize handles**

Add resize handles between grid cells for relative sizing.

**Step 5: Wire into MainContent**

**Step 6: Style**

**Step 7: Run the build**

Run: `cd web && bun run build`
Expected: Build succeeds.

**Step 8: Commit**

```bash
git add web/src/components/AutoGrid.tsx web/src/components/MainContent.tsx web/src/styles/terminal.css
git commit -m "feat(web): auto-grid tiled view with drag-to-swap and resize handles"
```

### Task 10: Quiescence Queue View

**Files:**
- Create: `web/src/components/QueueView.tsx`
- Modify: `web/src/components/MainContent.tsx`
- Modify: `web/src/app.tsx`

**Step 1: Build QueueView component**

Layout: top bar (pending queue thumbnails left, handled/active thumbnails right), center current session full-size, dismiss button.

```typescript
interface QueueViewProps {
  sessions: string[];
  groupTag: string;
  client: WshClient;
}
```

**Step 2: Wire quiescence subscription**

On entering queue mode for a group, start `await_quiesce` calls for all sessions in the group. When a session becomes quiescent, add it to the group's queue via `enqueueSession()`. On dismiss, call `dismissQueueEntry()` and re-subscribe to quiescence for that session.

**Step 3: Build the top bar strip**

Small thumbnails using `MiniTerminal`. Pending queue on left with count, handled/active on right with muted styling.

**Step 4: Build the dismiss action**

Prominent checkmark button + Super+Enter keyboard shortcut.

**Step 5: Empty state**

"All caught up" message when queue is empty.

**Step 6: Wire into MainContent**

**Step 7: Style**

**Step 8: Run the build**

Run: `cd web && bun run build`
Expected: Build succeeds.

**Step 9: Commit**

```bash
git add web/src/components/QueueView.tsx web/src/components/MainContent.tsx web/src/app.tsx web/src/styles/terminal.css
git commit -m "feat(web): quiescence queue view with dismiss and FIFO ordering"
```

### Task 11: View Mode Toggle

**Files:**
- Modify: `web/src/components/MainContent.tsx`

**Step 1: Build view mode toggle**

Three icon buttons in the main header: carousel, tiles, queue. Active mode highlighted. Clicking switches the mode for the current group.

**Step 2: Wire keyboard shortcuts**

Super+F = carousel, Super+G = tiled, Super+Q = queue.

**Step 3: Style**

**Step 4: Commit**

```bash
git add web/src/components/MainContent.tsx web/src/styles/terminal.css
git commit -m "feat(web): view mode toggle with keyboard shortcuts"
```

---

## Phase 5: Drag & Drop Tag Management

### Task 12: Drag and Drop System

**Files:**
- Create: `web/src/hooks/useDragDrop.ts`
- Modify: `web/src/components/Sidebar.tsx`
- Modify: `web/src/components/AutoGrid.tsx`
- Modify: `web/src/components/DepthCarousel.tsx`

**Step 1: Build the drag-and-drop hook**

A shared hook that handles: drag initiation, tracking shift key state, visual feedback (label near cursor), and drop resolution. Uses HTML5 drag-and-drop API.

```typescript
interface DragState {
  type: "session" | "group";
  source: string;          // session name or group tag
  sourceTag?: string;      // current tag of dragged session (for remove on move)
  shiftHeld: boolean;      // add vs move
}

export function useDragDrop(client: WshClient) {
  // ... drag state, event handlers, shift key tracking ...
}
```

**Step 2: Make sidebar groups drop targets**

When a session is dropped on a sidebar group:
- Default: remove all tags, add target group's tag
- Shift: add target group's tag, keep existing

Call `client.updateSession()` with appropriate `add_tags` and `remove_tags`.

**Step 3: Make sessions draggable**

From: tiled view cells, carousel side previews, queue top bar thumbnails, sidebar mini-previews.

**Step 4: Add visual feedback**

- Floating label near cursor: "Move to [tag]" / "Also add to [tag]"
- Drop target highlight (border glow)
- Invalid target shows no-drop cursor

**Step 5: Handle group dissolution**

When the last session leaves a tag group, the group disappears (this happens automatically since groups are computed from `sessionInfoMap`).

**Step 6: Style the drag feedback**

**Step 7: Run the build**

Run: `cd web && bun run build`
Expected: Build succeeds.

**Step 8: Commit**

```bash
git add web/src/hooks/useDragDrop.ts web/src/components/Sidebar.tsx web/src/components/AutoGrid.tsx web/src/components/DepthCarousel.tsx web/src/styles/terminal.css
git commit -m "feat(web): drag-and-drop tag management with move/add modes"
```

---

## Phase 6: Command Palette & Keyboard Help

### Task 13: Command Palette (Super+K)

**Files:**
- Create: `web/src/components/CommandPalette.tsx`
- Modify: `web/src/app.tsx`

**Step 1: Build the CommandPalette component**

Modal overlay with: text input (auto-focused), filtered results list, category labels. Fuzzy matching across sessions (by name), groups/tags, and actions.

```typescript
interface PaletteItem {
  type: "session" | "group" | "action";
  label: string;
  description?: string;
  action: () => void;
}
```

Built-in actions: new session, kill focused session, switch theme (sub-items for each theme), toggle sidebar, carousel/tiled/queue mode.

**Step 2: Wire Super+K shortcut**

In `app.tsx`, listen for the keyboard event and toggle palette visibility.

**Step 3: Implement fuzzy matching**

Simple substring match with scoring: exact prefix > contains > fuzzy. Sessions ranked first, then groups, then actions.

**Step 4: Style**

Centered modal, dark backdrop, smooth fade-in. Styled to match current theme.

**Step 5: Run the build**

Run: `cd web && bun run build`
Expected: Build succeeds.

**Step 6: Commit**

```bash
git add web/src/components/CommandPalette.tsx web/src/app.tsx web/src/styles/terminal.css
git commit -m "feat(web): command palette with fuzzy search (Super+K)"
```

### Task 14: Keyboard Shortcut Cheat Sheet (Super+?)

**Files:**
- Create: `web/src/components/ShortcutSheet.tsx`
- Modify: `web/src/app.tsx`

**Step 1: Build the ShortcutSheet component**

Modal overlay organized by category. Searchable: type to filter shortcuts.

**Step 2: Wire Super+? shortcut and ? icon in sidebar**

**Step 3: Style**

**Step 4: Commit**

```bash
git add web/src/components/ShortcutSheet.tsx web/src/app.tsx web/src/styles/terminal.css
git commit -m "feat(web): keyboard shortcut cheat sheet (Super+?)"
```

### Task 15: Global Keyboard Shortcuts

**Files:**
- Create: `web/src/hooks/useKeyboardShortcuts.ts`
- Modify: `web/src/app.tsx`

**Step 1: Build centralized keyboard shortcut handler**

A hook that registers all Super+X and Ctrl+Shift+X shortcuts in one place. Handles: carousel rotate, dismiss queue, jump to Nth session, cycle groups, toggle sidebar, theme picker, new session, kill session, view modes, tile focus movement, command palette, shortcut sheet.

**Step 2: Wire into app.tsx**

Replace the current ad-hoc keyboard listeners.

**Step 3: Run the build**

Run: `cd web && bun run build`
Expected: Build succeeds.

**Step 4: Commit**

```bash
git add web/src/hooks/useKeyboardShortcuts.ts web/src/app.tsx
git commit -m "feat(web): centralized keyboard shortcuts with Super and Ctrl+Shift modifiers"
```

---

## Phase 7: Themes

### Task 16: New Themes (Tokyo Night, Catppuccin, Dracula)

**Files:**
- Modify: `web/src/styles/themes.css`
- Modify: `web/src/app.tsx` (theme class sync)

**Step 1: Add Tokyo Night theme**

Define CSS variables for Tokyo Night: `--bg: #1a1b26`, `--fg: #a9b1d6`, accent `#7aa2f7`, plus all 16 ANSI colors using the Tokyo Night palette. Add component-specific styles following the same pattern as existing themes.

**Step 2: Add Catppuccin Mocha theme**

Define CSS variables: `--bg: #1e1e2e`, `--fg: #cdd6f4`, accent `#cba6f7`. All 16 ANSI from Catppuccin Mocha palette.

**Step 3: Add Dracula theme**

Define CSS variables: `--bg: #282a36`, `--fg: #f8f8f2`, accent `#bd93f9`. All 16 ANSI from Dracula palette.

**Step 4: Update theme class sync in app.tsx**

Add the new theme class names to the `classList.remove()` call.

**Step 5: Add new theme styles for all new components**

Sidebar, layout shell, command palette, queue view, etc. all need theme-specific styles.

**Step 6: Run the build**

Run: `cd web && bun run build`
Expected: Build succeeds.

**Step 7: Commit**

```bash
git add web/src/styles/themes.css web/src/app.tsx
git commit -m "feat(web): add Tokyo Night, Catppuccin Mocha, and Dracula themes"
```

### Task 17: Theme Polish Pass

**Files:**
- Modify: `web/src/styles/themes.css`
- Modify: `web/src/styles/terminal.css`

**Step 1: Consistent border-radius**

Audit all components and ensure consistent `border-radius` (e.g., 8px for panels, 6px for buttons, 4px for inputs).

**Step 2: Styled scrollbars**

Add themed scrollbar styles using `::-webkit-scrollbar` and `scrollbar-color` for Firefox.

**Step 3: Hover and focus states**

Ensure every interactive element has a hover state and a visible focus indicator that matches the theme.

**Step 4: Transitions everywhere**

Add `transition` to all elements that change on theme switch, view mode switch, group selection, etc.

**Step 5: Reduced motion support**

Add `@media (prefers-reduced-motion: reduce)` that disables carousel 3D transitions, fade animations, and replaces with instant swaps.

**Step 6: Commit**

```bash
git add web/src/styles/themes.css web/src/styles/terminal.css
git commit -m "style(web): polish pass - consistent radii, styled scrollbars, transitions, reduced motion"
```

---

## Phase 8: Mobile Adaptation

### Task 18: Mobile Bottom Sheet & Responsive Layout

**Files:**
- Create: `web/src/components/BottomSheet.tsx`
- Modify: `web/src/components/LayoutShell.tsx`
- Modify: `web/src/styles/terminal.css`

**Step 1: Create BottomSheet component**

Replaces the sidebar on mobile (< 640px). Swipe-up from a tab bar to reveal the group list. Tab bar shows: current group name, badge count, "+" button.

**Step 2: Update LayoutShell for responsive breakpoints**

Use `window.matchMedia` or CSS media queries:
- `< 640px`: bottom sheet, stacked tiles, no carousel side previews
- `640-1024px`: overlay sidebar (floats over main content), 2-column tiles
- `> 1024px`: persistent sidebar

**Step 3: Mobile carousel adaptation**

Full-width, no side previews. Edge shadows to hint at more sessions.

**Step 4: Mobile tiled adaptation**

Vertical stack (sessions stacked full-width, scrollable).

**Step 5: Mobile queue adaptation**

Compact strip top bar with count badge (not thumbnails). FAB for dismiss.

**Step 6: Mobile drag-and-drop**

Long-press to initiate. Bottom sheet auto-expands. "Move / Add" toggle pill replaces Shift modifier.

**Step 7: Style all mobile breakpoints**

**Step 8: Run the build**

Run: `cd web && bun run build`
Expected: Build succeeds.

**Step 9: Commit**

```bash
git add web/src/components/BottomSheet.tsx web/src/components/LayoutShell.tsx web/src/styles/terminal.css
git commit -m "feat(web): mobile adaptation with bottom sheet, responsive breakpoints"
```

---

## Phase 9: Cleanup & Documentation

### Task 19: Remove Old Components

**Files:**
- Delete: `web/src/components/SessionCarousel.tsx`
- Delete: `web/src/components/SessionGrid.tsx`
- Delete: `web/src/components/SessionThumbnail.tsx`
- Delete: `web/src/components/TiledLayout.tsx`
- Delete: `web/src/components/StatusBar.tsx`
- Delete: `web/src/components/PageIndicator.tsx`

**Step 1: Remove the files**

**Step 2: Remove any remaining imports of these components**

**Step 3: Clean up unused state (old tileLayout, tileSelection signals if still present)**

**Step 4: Run the build**

Run: `cd web && bun run build`
Expected: Build succeeds with no dead code warnings.

**Step 5: Commit**

```bash
git add -A web/src/
git commit -m "chore(web): remove old carousel, grid, thumbnail, tiled, status bar components"
```

### Task 20: Extract Shared Terminal Utilities

**Files:**
- Create: `web/src/utils/terminal.ts`
- Modify: `web/src/components/Terminal.tsx`
- Modify: `web/src/components/MiniTerminal.tsx`

**Step 1: Extract colorToCSS, spanStyle, renderLine into shared utils**

Both `Terminal.tsx` and `MiniTerminal.tsx` need to render styled terminal content. Extract the shared rendering utilities.

**Step 2: Update imports in both components**

**Step 3: Run the build**

Run: `cd web && bun run build`
Expected: Build succeeds.

**Step 4: Commit**

```bash
git add web/src/utils/terminal.ts web/src/components/Terminal.tsx web/src/components/MiniTerminal.tsx
git commit -m "refactor(web): extract shared terminal rendering utilities"
```

### Task 21: Accessibility Pass

**Files:**
- Modify: Various component files

**Step 1: Add ARIA labels**

- Sidebar groups: `role="button"`, `aria-label`, `aria-selected`
- View mode toggle: `role="radiogroup"`, `aria-checked`
- Session status badges: `aria-label` with status text
- Command palette: `role="dialog"`, `aria-label`

**Step 2: Add aria-live for status changes**

Session status changes announced via `aria-live="polite"` region.

**Step 3: High-contrast theme**

Add a "High Contrast" theme option that meets WCAG AA ratios.

**Step 4: Commit**

```bash
git add web/src/
git commit -m "a11y(web): add ARIA labels, live regions, and high-contrast theme"
```

### Task 22: Update Documentation and Skills

**Files:**
- Modify: `docs/API.md` (if web UI section exists)
- Modify: Skills files (if they reference the web UI)
- Modify: `README.md` (if it references the web UI)

**Step 1: Update any documentation that references the old UI layout**

**Step 2: Document the new view modes, sidebar, keyboard shortcuts, themes**

**Step 3: Commit**

```bash
git add docs/ skills/ README.md
git commit -m "docs: update documentation for web UI redesign"
```
