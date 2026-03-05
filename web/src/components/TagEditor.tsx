import { useState, useRef, useEffect, useCallback } from "preact/hooks";
import type { WshClient } from "../api/ws";
import { sessionInfoMap } from "../state/sessions";

interface TagEditorProps {
  /** Session name to edit tags for */
  session: string;
  /** The WshClient for API calls */
  client: WshClient;
  /** Close the popover */
  onClose: () => void;
}

export function TagEditor({ session, client, onClose }: TagEditorProps) {
  const [input, setInput] = useState("");
  const [suggestions, setSuggestions] = useState<string[]>([]);
  const inputRef = useRef<HTMLInputElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);

  const info = sessionInfoMap.value.get(session);
  const currentTags = info?.tags ?? [];

  // Collect all existing tags for autocomplete
  const allTags = Array.from(
    new Set(
      Array.from(sessionInfoMap.value.values()).flatMap((s) => s.tags)
    )
  ).sort();

  // Focus input on mount
  useEffect(() => {
    // Small delay to ensure the popover is positioned and visible before focusing
    const timer = setTimeout(() => inputRef.current?.focus(), 16);
    return () => clearTimeout(timer);
  }, []);

  // Close on click outside
  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        onClose();
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [onClose]);

  // Close on Escape
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  }, [onClose]);

  // Update suggestions when input changes
  useEffect(() => {
    if (input.trim() === "") {
      setSuggestions([]);
      return;
    }
    const lower = input.toLowerCase();
    const filtered = allTags.filter(
      (t) => t.toLowerCase().includes(lower) && !currentTags.includes(t)
    );
    setSuggestions(filtered.slice(0, 5));
  }, [input]);

  const addTag = useCallback((tag: string) => {
    const trimmed = tag.trim();
    if (!trimmed || currentTags.includes(trimmed)) return;
    client.updateTags(session, [trimmed]).catch((e) => {
      console.error("Failed to add tag:", e);
    });
    setInput("");
    setSuggestions([]);
  }, [client, session, currentTags]);

  const removeTag = useCallback((tag: string) => {
    client.updateTags(session, undefined, [tag]).catch((e) => {
      console.error("Failed to remove tag:", e);
    });
  }, [client, session]);

  const handleKeyDown = useCallback((e: KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      e.stopPropagation();
      if (input.trim()) {
        addTag(input);
      } else {
        // Enter on empty input dismisses
        onClose();
      }
    } else if (e.key === "Tab" || e.key === ",") {
      if (input.trim()) {
        e.preventDefault();
        addTag(input);
      }
    }
  }, [input, addTag, onClose]);

  return (
    <div class="tag-editor" ref={containerRef} onClick={(e: MouseEvent) => e.stopPropagation()}>
      <div class="tag-editor-header">Tags</div>
      {currentTags.length > 0 ? (
        <div class="tag-editor-tags">
          {currentTags.map((tag) => (
            <span key={tag} class="tag-editor-tag">
              <span class="tag-editor-tag-label">{tag}</span>
              <button
                class="tag-editor-remove"
                onClick={() => removeTag(tag)}
                title={`Remove "${tag}"`}
              >
                ×
              </button>
            </span>
          ))}
        </div>
      ) : (
        <div class="tag-editor-empty">No tags</div>
      )}
      <div class="tag-editor-input-wrap">
        <input
          ref={inputRef}
          type="text"
          class="tag-editor-input"
          placeholder="Type to add..."
          value={input}
          onInput={(e) => setInput((e.target as HTMLInputElement).value)}
          onKeyDown={handleKeyDown}
        />
      </div>
      {suggestions.length > 0 && (
        <div class="tag-editor-suggestions">
          {suggestions.map((s) => (
            <button key={s} class="tag-editor-suggestion" onClick={() => addTag(s)}>
              {s}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
