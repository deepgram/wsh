/**
 * WebSocket client for the orchestrator queue server.
 * Receives real-time queue updates and can resolve queue entries.
 */

export interface QueueEntry {
  id: string;
  project_id: string;
  session_name: string;
  actor: string;
  kind: string;
  text: string;
  ts: string;
  refs: Record<string, unknown>;
  human_attention_needed: boolean;
}

export type QueueEvent =
  | { type: "queue_snapshot"; entries: QueueEntry[] }
  | { type: "queue_add"; entry: QueueEntry }
  | { type: "queue_remove"; id: string };

type QueueChangeCallback = (entries: QueueEntry[]) => void;

export class OrchestratorClient {
  private ws: WebSocket | null = null;
  private baseUrl: string;
  private wsUrl: string;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private reconnectDelay = 1000;
  private entries: QueueEntry[] = [];

  onChange?: QueueChangeCallback;
  onConnectionChange?: (connected: boolean) => void;

  constructor(baseUrl: string) {
    this.baseUrl = baseUrl.replace(/\/$/, "");
    const wsProto = location.protocol === "https:" ? "wss:" : "ws:";
    // For dev proxy, we use the same host with /orch-ws path
    this.wsUrl = `${wsProto}//${location.host}/orch-ws`;
  }

  connect(): void {
    this.doConnect();
  }

  disconnect(): void {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }
  }

  getEntries(): QueueEntry[] {
    return this.entries;
  }

  async resolve(
    entryId: string,
    action: string,
    text?: string,
  ): Promise<void> {
    const body: Record<string, string> = { action };
    if (text) body.text = text;

    const resp = await fetch(`${this.baseUrl}/queue/${entryId}/resolve`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });

    if (!resp.ok) {
      throw new Error(`Resolve failed: ${resp.status}`);
    }
  }

  private doConnect(): void {
    const ws = new WebSocket(this.wsUrl);

    ws.onopen = () => {
      this.ws = ws;
      this.reconnectDelay = 1000;
      this.onConnectionChange?.(true);
    };

    ws.onmessage = (ev) => {
      try {
        const msg = JSON.parse(ev.data as string) as QueueEvent;
        this.handleEvent(msg);
      } catch (e) {
        console.error("Orchestrator WS parse error:", e);
      }
    };

    ws.onclose = () => {
      this.ws = null;
      this.onConnectionChange?.(false);
      this.scheduleReconnect();
    };

    ws.onerror = () => {};
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer) return;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.reconnectDelay = Math.min(this.reconnectDelay * 2, 10000);
      this.doConnect();
    }, this.reconnectDelay);
  }

  private handleEvent(msg: QueueEvent): void {
    switch (msg.type) {
      case "queue_snapshot":
        this.entries = msg.entries;
        break;
      case "queue_add":
        // Add to front (newest first)
        if (!this.entries.some((e) => e.id === msg.entry.id)) {
          this.entries = [msg.entry, ...this.entries];
        }
        break;
      case "queue_remove":
        this.entries = this.entries.filter((e) => e.id !== msg.id);
        break;
    }
    this.onChange?.(this.entries);
  }
}
