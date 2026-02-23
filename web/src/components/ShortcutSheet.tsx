import { useState, useRef, useEffect, useCallback } from "preact/hooks";

interface Shortcut {
  keys: string;
  description: string;
}

interface ShortcutCategory {
  label: string;
  shortcuts: Shortcut[];
}

const CATEGORIES: ShortcutCategory[] = [
  {
    label: "Navigation",
    shortcuts: [
      { keys: "Ctrl+Shift+Left/Right or H/L", description: "Navigate sessions (all views)" },
      { keys: "Ctrl+Shift+Up/Down or K/J", description: "Grid navigate rows" },
      { keys: "Ctrl+Shift+1-9", description: "Jump to Nth session" },
      { keys: "Ctrl+Shift+Tab", description: "Next sidebar group" },
    ],
  },
  {
    label: "View Modes",
    shortcuts: [
      { keys: "Ctrl+Shift+F", description: "Carousel mode" },
      { keys: "Ctrl+Shift+G", description: "Tiled mode" },
      { keys: "Ctrl+Shift+Q", description: "Queue mode" },
    ],
  },
  {
    label: "Session Management",
    shortcuts: [
      { keys: "Ctrl+Shift+O", description: "New session" },
      { keys: "Ctrl+Shift+W", description: "Kill focused session" },
      { keys: "Ctrl+Shift+Enter", description: "Dismiss & next idle session (queue)" },
    ],
  },
  {
    label: "UI",
    shortcuts: [
      { keys: "Ctrl+Shift+B", description: "Toggle sidebar" },
      { keys: "Ctrl+Shift+K", description: "Command palette" },
      { keys: "Ctrl+Shift+/", description: "This help" },
    ],
  },
];

interface ShortcutSheetProps {
  onClose: () => void;
}

export function ShortcutSheet({ onClose }: ShortcutSheetProps) {
  const [filter, setFilter] = useState("");
  const containerRef = useRef<HTMLDivElement>(null);

  // Close on Escape
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, [onClose]);

  // Delegate typing to filter input when user starts typing in the dialog
  const handleSheetKeyDown = useCallback((e: KeyboardEvent) => {
    if (e.key.length === 1 && !e.ctrlKey && !e.altKey && !e.metaKey) {
      const filterInput = containerRef.current?.querySelector(".shortcut-filter") as HTMLInputElement | null;
      if (filterInput && document.activeElement !== filterInput) {
        filterInput.focus();
      }
    }
  }, []);

  const lower = filter.toLowerCase();
  const filteredCategories = CATEGORIES.map((cat) => ({
    ...cat,
    shortcuts: cat.shortcuts.filter(
      (s) =>
        s.keys.toLowerCase().includes(lower) ||
        s.description.toLowerCase().includes(lower)
    ),
  })).filter((cat) => cat.shortcuts.length > 0);

  return (
    <div class="shortcut-backdrop" onClick={onClose}>
      <div class="shortcut-sheet" ref={containerRef} onKeyDown={handleSheetKeyDown} onClick={(e: MouseEvent) => e.stopPropagation()} role="dialog" aria-label="Keyboard shortcuts">
        <div class="shortcut-sheet-header">
          <span class="shortcut-sheet-title">Keyboard Shortcuts</span>
          <button class="shortcut-sheet-close" onClick={onClose}>&times;</button>
        </div>
        <input
          class="shortcut-filter"
          type="text"
          placeholder="Filter shortcuts..."
          value={filter}
          onInput={(e) => setFilter((e.target as HTMLInputElement).value)}
        />
        <div class="shortcut-categories">
          {filteredCategories.map((cat) => (
            <div key={cat.label} class="shortcut-category">
              <div class="shortcut-category-label">{cat.label}</div>
              {cat.shortcuts.map((s) => (
                <div key={s.keys} class="shortcut-row">
                  <kbd class="shortcut-keys">{s.keys}</kbd>
                  <span class="shortcut-desc">{s.description}</span>
                </div>
              ))}
            </div>
          ))}
          {filteredCategories.length === 0 && (
            <div class="shortcut-empty">No matching shortcuts</div>
          )}
        </div>
        <div class="shortcut-footer">
          All shortcuts use Ctrl+Shift as the modifier.
        </div>
      </div>
    </div>
  );
}
