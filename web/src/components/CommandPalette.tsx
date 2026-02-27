import { useState, useRef, useEffect, useCallback, useMemo } from "preact/hooks";
import { sessions, focusedSession, sessionInfoMap, sidebarCollapsed, setTheme, type Theme } from "../state/sessions";
import { groups, selectedGroups, setViewModeForGroup, setGroupBy } from "../state/groups";
import type { WshClient } from "../api/ws";

interface PaletteItem {
  type: "session" | "group" | "action" | "server";
  label: string;
  description?: string;
  shortcut?: string;
  action: () => void;
}

interface CommandPaletteProps {
  client: WshClient;
  onClose: () => void;
  onShowServers?: () => void;
}

function fuzzyMatch(query: string, text: string): number {
  const lower = query.toLowerCase();
  const target = text.toLowerCase();

  // Exact prefix match scores highest
  if (target.startsWith(lower)) return 3;
  // Contains match
  if (target.includes(lower)) return 2;
  // Fuzzy: all chars present in order
  let qi = 0;
  for (let ti = 0; ti < target.length && qi < lower.length; ti++) {
    if (target[ti] === lower[qi]) qi++;
  }
  return qi === lower.length ? 1 : 0;
}

export function CommandPalette({ client, onClose, onShowServers }: CommandPaletteProps) {
  const [query, setQuery] = useState("");
  const [selectedIndex, setSelectedIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const onCloseRef = useRef(onClose);
  useEffect(() => { onCloseRef.current = onClose; });

  // Focus on mount + close on Escape (capture phase, registered once)
  useEffect(() => {
    inputRef.current?.focus();
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onCloseRef.current();
      }
    };
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, []);

  // Build items
  const items = useMemo((): PaletteItem[] => {
    const result: PaletteItem[] = [];

    // Sessions
    for (const name of sessions.value) {
      const info = sessionInfoMap.value.get(name);
      result.push({
        type: "session",
        label: name,
        description: info ? `${info.command || "shell"} — ${info.tags.length ? info.tags.join(", ") : "no tags"}` : undefined,
        action: () => { focusedSession.value = name; onClose(); },
      });
    }

    // Groups
    for (const g of groups.value) {
      if (g.tag === "all") continue; // Skip "all" in palette
      result.push({
        type: "group",
        label: g.label,
        description: `${g.sessions.length} sessions`,
        action: () => { selectedGroups.value = [g.tag]; onClose(); },
      });
    }

    // Servers — derive unique server list from session info
    const serverCounts = new Map<string, number>();
    for (const info of sessionInfoMap.value.values()) {
      if (info.server) {
        serverCounts.set(info.server, (serverCounts.get(info.server) || 0) + 1);
      }
    }
    for (const [server, count] of Array.from(serverCounts.entries()).sort()) {
      result.push({
        type: "server",
        label: `Go to server: ${server}`,
        description: `${count} session${count !== 1 ? "s" : ""}`,
        action: () => {
          setGroupBy("server");
          selectedGroups.value = [`server:${server}`];
          onClose();
        },
      });
    }

    // Actions
    result.push({
      type: "action",
      label: "New Session",
      description: "Create a new terminal session",
      shortcut: "Ctrl+Shift+O",
      action: () => { client.createSession().catch(() => {}); onClose(); },
    });

    result.push({
      type: "action",
      label: "Toggle Sidebar",
      shortcut: "Ctrl+Shift+B",
      action: () => { sidebarCollapsed.value = !sidebarCollapsed.value; onClose(); },
    });

    if (onShowServers) {
      result.push({
        type: "action",
        label: "Show Servers",
        description: "View federated backend servers and their status",
        action: () => { onShowServers(); },
      });
    }

    const themes: { id: Theme; label: string }[] = [
      { id: "glass", label: "Glass" },
      { id: "neon", label: "Neon" },
      { id: "minimal", label: "Minimal" },
      { id: "tokyo-night", label: "Tokyo Night" },
      { id: "catppuccin", label: "Catppuccin" },
      { id: "dracula", label: "Dracula" },
      { id: "high-contrast", label: "High Contrast" },
    ];
    for (const t of themes) {
      result.push({
        type: "action",
        label: `Theme: ${t.label}`,
        action: () => { setTheme(t.id); onClose(); },
      });
    }

    const modes = [
      { mode: "carousel" as const, label: "Carousel View", shortcut: "Ctrl+Shift+F" },
      { mode: "tiled" as const, label: "Tiled View", shortcut: "Ctrl+Shift+G" },
      { mode: "queue" as const, label: "Queue View", shortcut: "Ctrl+Shift+Q" },
    ];
    for (const m of modes) {
      result.push({
        type: "action",
        label: m.label,
        shortcut: m.shortcut,
        action: () => {
          const tag = selectedGroups.value[0] || "all";
          setViewModeForGroup(tag, m.mode);
          onClose();
        },
      });
    }

    // Tag session actions
    for (const name of sessions.value) {
      result.push({
        type: "action",
        label: `Tag: ${name}`,
        description: "Add or remove tags for this session",
        action: () => {
          const tag = prompt(`Enter tag for session "${name}":`);
          if (tag?.trim()) {
            client.updateSession(name, { add_tags: [tag.trim()] }).catch(() => {});
          }
          onClose();
        },
      });
    }

    return result;
  }, [client, onClose, onShowServers]);

  // Filter items
  const filtered = useMemo(() => {
    if (!query.trim()) return items;
    return items
      .map((item) => ({ item, score: fuzzyMatch(query, item.label) }))
      .filter((x) => x.score > 0)
      .sort((a, b) => {
        // Sort by type priority then score
        const typePriority: Record<string, number> = { session: 0, group: 1, server: 2, action: 3 };
        const tp = (typePriority[a.item.type] ?? 9) - (typePriority[b.item.type] ?? 9);
        if (tp !== 0) return tp;
        return b.score - a.score;
      })
      .map((x) => x.item);
  }, [query, items]);

  // Reset selection when filter changes
  useEffect(() => {
    setSelectedIndex(0);
  }, [filtered.length]);

  // Keyboard navigation
  const handleKeyDown = useCallback((e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onClose();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setSelectedIndex((i) => Math.min(i + 1, filtered.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSelectedIndex((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const item = filtered[selectedIndex];
      if (item) item.action();
    }
  }, [filtered, selectedIndex, onClose]);

  // Scroll selected item into view
  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const selected = list.children[selectedIndex] as HTMLElement | undefined;
    selected?.scrollIntoView({ block: "nearest" });
  }, [selectedIndex]);

  return (
    <div class="palette-backdrop" onClick={onClose}>
      <div class="palette" role="dialog" aria-label="Command palette" onClick={(e: MouseEvent) => e.stopPropagation()}>
        <input
          ref={inputRef}
          class="palette-input"
          type="text"
          placeholder="Type to search sessions, groups, actions..."
          value={query}
          onInput={(e) => setQuery((e.target as HTMLInputElement).value)}
          onKeyDown={handleKeyDown}
          aria-label="Search commands"
        />
        <div class="palette-list" ref={listRef} role="listbox">
          {filtered.length === 0 && (
            <div class="palette-empty">No results</div>
          )}
          {filtered.map((item, i) => (
            <div
              key={`${item.type}-${item.label}`}
              class={`palette-item ${i === selectedIndex ? "selected" : ""}`}
              onClick={() => item.action()}
              onMouseEnter={() => setSelectedIndex(i)}
              role="option"
              aria-selected={i === selectedIndex}
            >
              <span class={`palette-type-badge palette-type-${item.type}`}>
                {item.type === "session" ? "S" : item.type === "group" ? "G" : item.type === "server" ? "H" : "A"}
              </span>
              <div class="palette-item-text">
                <span class="palette-item-label">{item.label}</span>
                {item.description && (
                  <span class="palette-item-desc">{item.description}</span>
                )}
              </div>
              {item.shortcut && (
                <kbd class="palette-shortcut">{item.shortcut}</kbd>
              )}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
