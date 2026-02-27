import { useState, useEffect, useCallback } from "preact/hooks";
import type { WshClient } from "../api/ws";
import type { ServerInfo } from "../api/types";
import { sessionInfoMap } from "../state/sessions";

interface ServerListProps {
  client: WshClient;
  onClose: () => void;
}

function healthDotClass(health: string): string {
  switch (health) {
    case "healthy":
      return "server-health-green";
    case "connecting":
      return "server-health-yellow";
    default:
      return "server-health-red";
  }
}

export function ServerList({ client, onClose }: ServerListProps) {
  const [servers, setServers] = useState<ServerInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const fetchServers = useCallback(() => {
    setLoading(true);
    setError(null);
    client.listServers()
      .then((result) => {
        // Enrich session counts from local session info if not present
        const infoMap = sessionInfoMap.value;
        const enriched = result.map((s) => {
          if (s.sessions !== undefined) return s;
          // Count sessions from sessionInfoMap for this server
          let count = 0;
          for (const info of infoMap.values()) {
            if (info.server === s.hostname) count++;
          }
          // For local server (no server field), count sessions without server
          if (s.address === "local") {
            for (const info of infoMap.values()) {
              if (!info.server) count++;
            }
          }
          return { ...s, sessions: count };
        });
        setServers(enriched);
        setLoading(false);
      })
      .catch((e) => {
        setError(e.message);
        setLoading(false);
      });
  }, [client]);

  useEffect(() => {
    fetchServers();
  }, [fetchServers]);

  return (
    <div class="server-list-backdrop" onClick={onClose}>
      <div
        class="server-list-panel"
        role="dialog"
        aria-label="Server list"
        onClick={(e: MouseEvent) => e.stopPropagation()}
      >
        <div class="server-list-header">
          <span class="server-list-title">Servers</span>
          <button class="server-list-refresh" onClick={fetchServers} title="Refresh">
            &#8635;
          </button>
          <button class="server-list-close" onClick={onClose} title="Close">
            &times;
          </button>
        </div>
        <div class="server-list-body">
          {loading && <div class="server-list-loading">Loading...</div>}
          {error && <div class="server-list-error">{error}</div>}
          {!loading && !error && servers.length === 0 && (
            <div class="server-list-empty">No servers configured</div>
          )}
          {!loading && !error && servers.map((s) => (
            <div key={s.hostname} class="server-list-item">
              <span class={`server-health-dot ${healthDotClass(s.health)}`} />
              <div class="server-list-item-info">
                <span class="server-list-hostname">{s.hostname}</span>
                <span class="server-list-address">{s.address}</span>
              </div>
              <span class="server-list-sessions">
                {s.sessions !== undefined ? `${s.sessions} sessions` : ""}
              </span>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
