import { Terminal } from "./Terminal";

interface SessionThumbnailProps {
  session: string;
  focused: boolean;
  selectionIndex: number; // -1 = not selected, 0+ = selection order
  onSelect: () => void;
  onClose: () => void;
  onToggleTile: () => void;
}

export function SessionThumbnail({
  session,
  focused,
  selectionIndex,
  onSelect,
  onClose,
  onToggleTile,
}: SessionThumbnailProps) {
  const selected = selectionIndex >= 0;

  return (
    <div
      class={`session-thumbnail ${focused ? "focused" : ""} ${selected ? "tile-selected" : ""}`}
      onClick={onSelect}
    >
      <div class="thumbnail-header">
        <span class="thumbnail-name">{session}</span>
        <div class="thumbnail-actions">
          <button
            class="thumbnail-close"
            onClick={(e) => {
              e.stopPropagation();
              onClose();
            }}
            title="Close session"
          >
            <svg width="10" height="10" viewBox="0 0 10 10">
              <path
                d="M1 1L9 9M9 1L1 9"
                stroke="currentColor"
                stroke-width="1.5"
                stroke-linecap="round"
              />
            </svg>
          </button>
        </div>
      </div>
      <div class="thumbnail-content">
        <Terminal session={session} />
      </div>
      <button
        class={`tile-select-circle ${selected ? "selected" : ""}`}
        onClick={(e) => {
          e.stopPropagation();
          onToggleTile();
        }}
        title={selected ? "Remove from tile" : "Add to tile"}
      >
        {selected ? selectionIndex + 1 : ""}
      </button>
    </div>
  );
}
