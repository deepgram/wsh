import type { WsRequest, WsResponse, EventType, ScreenResponse, ScrollbackResponse, SessionInfo } from "./types";

type PendingRequest = {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
};

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type EventCallback = (event: any) => void;

/**
 * Build a list of WebSocket URLs to try, ordered by preference.
 *
 * Browsers implement Happy Eyeballs (RFC 8305) for HTTP but Firefox
 * does not use it for WebSocket connections — it tries addresses
 * sequentially and waits for a full TCP timeout (~30-60s) before
 * falling back.  This is especially painful for "localhost" which
 * resolves to both ::1 and 127.0.0.1, and for SSH port-forwards
 * that typically only listen on IPv4.
 *
 * We work around this by racing multiple WebSocket connections
 * ourselves (client-side Happy Eyeballs): start the primary URL,
 * and after a short delay start fallbacks to both 127.0.0.1 and
 * [::1].  Whichever connects first wins; the losers are closed.
 *
 * We try both address families because SSH port-forwards may bind
 * to either IPv4 or IPv6 loopback depending on system configuration.
 */
function buildWsUrls(primary: string): string[] {
  const urls = [primary];
  try {
    const parsed = new URL(primary);
    if (parsed.hostname === "localhost") {
      const v4 = new URL(primary);
      v4.hostname = "127.0.0.1";
      urls.push(v4.toString());

      const v6 = new URL(primary);
      v6.hostname = "[::1]";
      urls.push(v6.toString());
    }
  } catch {
    // malformed URL — just use primary
  }
  return urls;
}

/** Delay before starting fallback connections (ms).  RFC 8305 recommends 250ms. */
const HAPPY_EYEBALLS_DELAY = 250;

export class WshClient {
  private ws: WebSocket | null = null;
  private nextId = 1;
  private pending = new Map<number, PendingRequest>();
  private eventCallbacks = new Map<string, Set<EventCallback>>();
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private reconnectDelay = 1000;
  private url = "";
  private token: string | null = null;

  onStateChange?: (state: "connecting" | "connected" | "disconnected") => void;

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  onLifecycleEvent?: (event: any) => void;

  onAuthRequired?: (reason: "needed" | "invalid") => void;

  setToken(token: string | null): void {
    this.token = token;
  }

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

  /** Append token query param to a URL if a token is configured. */
  private buildAuthUrl(url: string): string {
    if (!this.token) return url;
    const sep = url.includes("?") ? "&" : "?";
    return `${url}${sep}token=${encodeURIComponent(this.token)}`;
  }

  /** Derive an HTTP URL from a WebSocket URL for the auth probe. */
  private deriveHttpUrl(wsUrl: string): string {
    return wsUrl
      .replace(/^ws:/, "http:")
      .replace(/^wss:/, "https:")
      .replace(/\/ws\/json$/, "/sessions");
  }

  /**
   * Check whether a WebSocket connection failure is due to auth.
   *
   * The browser WebSocket API doesn't expose HTTP status codes on failure,
   * so we can't distinguish "401 Unauthorized" from "server down" from the
   * close event alone.  This method does a quick HTTP probe to find out.
   *
   * Returns true if auth is the problem (and fires onAuthRequired), false
   * if we should proceed with normal reconnect.
   */
  private async probeAuth(): Promise<boolean> {
    try {
      const httpUrl = this.deriveHttpUrl(this.url);
      const headers: Record<string, string> = {};
      if (this.token) {
        headers["Authorization"] = `Bearer ${this.token}`;
      }
      const controller = new AbortController();
      const timeout = setTimeout(() => controller.abort(), 5000);
      try {
        const resp = await fetch(httpUrl, { headers, signal: controller.signal });
        if (resp.status === 401) {
          this.onAuthRequired?.("needed");
          return true;
        }
        if (resp.status === 403) {
          this.onAuthRequired?.("invalid");
          return true;
        }
      } finally {
        clearTimeout(timeout);
      }
    } catch {
      // Network error or abort — not an auth problem
    }
    return false;
  }

  private doConnect(): void {
    this.onStateChange?.("connecting");

    // Probe auth upfront: the browser WebSocket API doesn't expose HTTP
    // status codes on failure, making it unreliable to distinguish a 401
    // from a network error via WS close events alone.  By probing the
    // HTTP API first we get a definitive signal before wasting time on
    // WebSocket connection attempts that will be rejected.
    this.probeAuth().then((isAuth) => {
      if (isAuth) return; // onAuthRequired already fired
      this.connectWebSocket();
    });
  }

  private connectWebSocket(): void {
    const urls = buildWsUrls(this.url);

    // Track all in-flight attempts so the winner can close the losers.
    const attempts: WebSocket[] = [];
    let settled = false;
    let fallbackTimer: ReturnType<typeof setTimeout> | null = null;
    let closedCount = 0;

    const settle = (winner: WebSocket) => {
      if (settled) {
        winner.close();
        return;
      }
      settled = true;
      if (fallbackTimer) clearTimeout(fallbackTimer);

      // Close all other attempts
      for (const ws of attempts) {
        if (ws !== winner) ws.close();
      }

      // Wire up the winner
      this.ws = winner;
      this.reconnectDelay = 1000;
      winner.onmessage = (ev) => {
        try {
          this.handleMessage(ev.data as string);
        } catch (e) {
          console.error("Error handling WebSocket message:", e);
        }
      };
      winner.onclose = () => {
        this.ws = null;
        this.rejectAllPending("WebSocket closed");
        this.onStateChange?.("disconnected");
        this.scheduleReconnect();
      };
      winner.onerror = () => {};
      this.onStateChange?.("connected");
    };

    const allFailed = () => {
      if (fallbackTimer) clearTimeout(fallbackTimer);
      this.onStateChange?.("disconnected");

      // WebSocket failed — probe again to distinguish auth failure from
      // server down.  (Auth state may have changed since the upfront probe.)
      this.probeAuth().then((isAuth) => {
        if (!isAuth) {
          this.scheduleReconnect();
        }
      });
    };

    const tryConnect = (url: string) => {
      const ws = new WebSocket(this.buildAuthUrl(url));
      attempts.push(ws);

      ws.onopen = () => settle(ws);
      ws.onerror = () => {};
      ws.onclose = () => {
        if (settled) return;
        closedCount++;
        if (closedCount >= attempts.length) {
          allFailed();
        }
      };
    };

    // Start primary connection immediately
    tryConnect(urls[0]);

    // Start fallback(s) after a short delay (Happy Eyeballs)
    if (urls.length > 1) {
      fallbackTimer = setTimeout(() => {
        if (settled) return;
        for (let i = 1; i < urls.length; i++) {
          tryConnect(urls[i]);
        }
      }, HAPPY_EYEBALLS_DELAY);
    }
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

    // Server hello — no action needed
    if ("connected" in msg) return;

    // Lagged notification — server's event buffer overflowed; a sync follows
    if ("type" in msg && msg.type === "lagged") {
      console.warn(
        `Event buffer lagged for session "${msg.session}", ${msg.skipped} events skipped`,
      );
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

  /** Re-key event callbacks from one session name to another (used on rename). */
  rekeySubscription(oldSession: string, newSession: string): void {
    const callbacks = this.eventCallbacks.get(oldSession);
    if (callbacks) {
      this.eventCallbacks.delete(oldSession);
      this.eventCallbacks.set(newSession, callbacks);
    }
  }

  // --- Convenience methods ---

  async createSession(name?: string, tags?: string[]): Promise<SessionInfo> {
    const params: Record<string, unknown> = {};
    if (name) params.name = name;
    if (tags && tags.length > 0) params.tags = tags;
    const result = await this.request("create_session", params);
    return result as SessionInfo;
  }

  async listSessions(tags?: string[]): Promise<SessionInfo[]> {
    const params = tags && tags.length > 0 ? { tag: tags } : undefined;
    const result = await this.request("list_sessions", params);
    return result as SessionInfo[];
  }

  async killSession(name: string): Promise<void> {
    await this.request("kill_session", { name });
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

  async resize(session: string, cols: number, rows: number): Promise<void> {
    await this.request("resize", { cols, rows }, session);
  }

  async getScrollback(
    session: string,
    offset: number,
    limit: number,
  ): Promise<ScrollbackResponse> {
    const result = await this.request(
      "get_scrollback",
      { format: "styled", offset, limit },
      session,
    );
    return result as ScrollbackResponse;
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
