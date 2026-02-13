import type { WsRequest, WsResponse, EventType, ScreenResponse } from "./types";

type PendingRequest = {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
};

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type EventCallback = (event: any) => void;

export class WshClient {
  private ws: WebSocket | null = null;
  private nextId = 1;
  private pending = new Map<number, PendingRequest>();
  private eventCallbacks = new Map<string, Set<EventCallback>>();
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private reconnectDelay = 1000;
  private url = "";

  onStateChange?: (state: "connecting" | "connected" | "disconnected") => void;

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  onLifecycleEvent?: (event: any) => void;

  connect(url: string): void {
    this.url = url;
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

  private doConnect(): void {
    this.onStateChange?.("connecting");
    const ws = new WebSocket(this.url);

    ws.onopen = () => {
      this.ws = ws;
      this.reconnectDelay = 1000;
      this.onStateChange?.("connected");
    };

    ws.onmessage = (ev) => {
      try {
        this.handleMessage(ev.data as string);
      } catch (e) {
        console.error("Error handling WebSocket message:", e);
      }
    };

    ws.onclose = () => {
      this.ws = null;
      this.rejectAllPending("WebSocket closed");
      this.onStateChange?.("disconnected");
      this.scheduleReconnect();
    };

    ws.onerror = () => {
      // onclose will fire after this
    };
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer) return;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.reconnectDelay = Math.min(this.reconnectDelay * 2, 10000);
      this.doConnect();
    }, this.reconnectDelay);
  }

  private rejectAllPending(reason: string): void {
    for (const [, req] of this.pending) {
      req.reject(new Error(reason));
    }
    this.pending.clear();
  }

  private handleMessage(data: string): void {
    let msg: Record<string, unknown>;
    try {
      msg = JSON.parse(data);
    } catch {
      return;
    }

    // If it has an id, it's a response to a request
    if ("id" in msg && msg.id != null) {
      const resp = msg as unknown as WsResponse;
      const pending = this.pending.get(resp.id as number);
      if (pending) {
        this.pending.delete(resp.id as number);
        if (resp.error) {
          pending.reject(new Error(`${resp.error.code}: ${resp.error.message}`));
        } else {
          pending.resolve(resp.result);
        }
      }
      return;
    }

    // Otherwise it's an event — route to callbacks
    if ("event" in msg) {
      const eventName = msg.event as string;

      // Lifecycle events (session_created, session_destroyed, etc.)
      if (eventName.startsWith("session_")) {
        try {
          this.onLifecycleEvent?.(msg);
        } catch (e) {
          console.error("Error in lifecycle event handler:", e);
        }
        return;
      }

      // Per-session events carry a "session" field from the server
      const session = msg.session as string | undefined;
      if (session) {
        const callbacks = this.eventCallbacks.get(session);
        if (callbacks) {
          for (const cb of callbacks) {
            try {
              cb(msg);
            } catch (e) {
              console.error(`Error in event callback for session "${session}":`, e);
            }
          }
        }
      } else {
        // No session field — broadcast to all (backward compat)
        for (const [, callbacks] of this.eventCallbacks) {
          for (const cb of callbacks) {
            try {
              cb(msg);
            } catch (e) {
              console.error("Error in event callback:", e);
            }
          }
        }
      }
    }
  }

  request(method: string, params?: unknown, session?: string): Promise<unknown> {
    return new Promise((resolve, reject) => {
      if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
        reject(new Error("Not connected"));
        return;
      }

      const id = this.nextId++;
      this.pending.set(id, { resolve, reject });

      const req: WsRequest = { id, method };
      if (session) req.session = session;
      if (params !== undefined) req.params = params;

      this.ws.send(JSON.stringify(req));
    });
  }

  /** Remove all local event subscriptions (used on reconnect). */
  clearAllSubscriptions(): void {
    this.eventCallbacks.clear();
  }

  // --- Convenience methods ---

  async createSession(name?: string): Promise<{ name: string }> {
    const result = await this.request("create_session", { name });
    return result as { name: string };
  }

  async listSessions(): Promise<Array<{ name: string }>> {
    const result = await this.request("list_sessions");
    return result as Array<{ name: string }>;
  }

  async killSession(name: string): Promise<void> {
    await this.request("kill_session", { name });
  }

  async getScreen(
    session: string,
    format: "plain" | "styled" = "styled",
  ): Promise<ScreenResponse> {
    const result = await this.request("get_screen", { format }, session);
    return result as ScreenResponse;
  }

  async sendInput(session: string, data: string): Promise<void> {
    await this.request("send_input", { data }, session);
  }

  subscribe(
    session: string,
    events: EventType[],
    callback: EventCallback,
  ): () => void {
    // Register callback locally
    let set = this.eventCallbacks.get(session);
    if (!set) {
      set = new Set();
      this.eventCallbacks.set(session, set);
    }
    set.add(callback);

    // Send subscribe request to server
    this.request("subscribe", { events, format: "styled" }, session).catch(
      () => {
        // Subscription failed — will retry on reconnect
      },
    );

    // Return unsubscribe function
    return () => {
      set!.delete(callback);
      if (set!.size === 0) {
        this.eventCallbacks.delete(session);
      }
    };
  }
}
