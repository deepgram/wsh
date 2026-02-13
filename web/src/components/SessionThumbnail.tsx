import { Terminal } from "./Terminal";

interface SessionThumbnailProps {
  session: string;
  focused: boolean;
  onSelect: () => void;
  onClose: () => void;
  onTile?: () => void;
}

export function SessionThumbnail({
  session,
  focused,
  onSelect,
  onClose,
  onTile,
}: SessionThumbnailProps) {
  return (
    <div
      class={`session-thumbnail ${focused ? "focused" : ""}`}
      onClick={onSelect}
    >
      <div class="thumbnail-header">
        <span class="thumbnail-name">{session}</span>
        <div class="thumbnail-actions">
          {onTile && (
            <button
              class="thumbnail-tile"
              onClick={(e) => {
                e.stopPropagation();
                onTile();
              }}
              title="Tile session"
            >
              <svg width="12" height="12" viewBox="0 0 12 12">
                <rect x="0" y="0" width="5" height="12" fill="currentColor" rx="1" />
                <rect x="7" y="0" width="5" height="12" fill="currentColor" rx="1" />
              </svg>
            </button>
          )}
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
    </div>
  );
}
