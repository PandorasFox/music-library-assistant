import type { WitchEvent } from "./generated/types";

export type WsState = "connecting" | "connected" | "disconnected";
export type WsListener = (event: WitchEvent) => void;
export type WsStateListener = (state: WsState) => void;

const MIN_BACKOFF_MS = 1000;
const MAX_BACKOFF_MS = 30000;

/** WebSocket manager: connects, reconnects with exponential backoff, parses WitchEvent JSON. */
export class WitchSocket {
  private ws: WebSocket | null = null;
  private backoff = MIN_BACKOFF_MS;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private intentionallyClosed = false;
  private listeners = new Set<WsListener>();
  private stateListeners = new Set<WsStateListener>();
  private _state: WsState = "disconnected";

  get state(): WsState {
    return this._state;
  }

  private setState(state: WsState): void {
    this._state = state;
    for (const fn of this.stateListeners) fn(state);
  }

  connect(): void {
    this.intentionallyClosed = false;
    this.tryConnect();
  }

  disconnect(): void {
    this.intentionallyClosed = true;
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }
    this.setState("disconnected");
  }

  onEvent(fn: WsListener): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  onStateChange(fn: WsStateListener): () => void {
    this.stateListeners.add(fn);
    return () => this.stateListeners.delete(fn);
  }

  private tryConnect(): void {
    this.setState("connecting");

    // Cookie-based auth — the browser sends mm_session automatically.
    // No token query param needed.
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    const url = `${proto}//${location.host}/ws`;
    const ws = new WebSocket(url);

    ws.onopen = () => {
      this.backoff = MIN_BACKOFF_MS;
      this.setState("connected");
    };

    ws.onmessage = (msg) => {
      try {
        const event = JSON.parse(msg.data as string) as WitchEvent;
        for (const fn of this.listeners) fn(event);
      } catch {
        // Malformed frame — skip.
      }
    };

    ws.onclose = () => {
      this.ws = null;
      if (!this.intentionallyClosed) {
        this.setState("disconnected");
        this.scheduleReconnect();
      }
    };

    ws.onerror = () => {
      // onclose will fire after this.
    };

    this.ws = ws;
  }

  private scheduleReconnect(): void {
    if (this.intentionallyClosed) return;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.tryConnect();
    }, this.backoff);
    this.backoff = Math.min(this.backoff * 2, MAX_BACKOFF_MS);
  }
}

/** Singleton instance shared across hooks. */
export const witchSocket = new WitchSocket();
