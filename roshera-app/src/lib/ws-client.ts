import { useWSStore } from '@/stores/ws-store'
import { parseServerMessage, type ServerMessage } from './ws-schemas'

type MessageHandler = (msg: ServerMessage) => void

const BASE_DELAY_MS = 1000
// Cap the backoff instead of capping the ATTEMPTS: a dev-server rebuild
// takes minutes, and the old 10-attempt limit gave up mid-rebuild and
// permanently stranded the tab (stale scene, dead UUIDs, deletes that
// silently no-op — the 2026-06-12 live-session failure). Retry forever,
// at most every MAX_DELAY_MS.
const MAX_DELAY_MS = 10_000
const HEARTBEAT_INTERVAL_MS = 30_000

/// Installed by ws-bridge: full scene refetch from `/api/scene/snapshot`.
/// Called on every RE-connect (not the first connect) because the server
/// may have restarted — everything the client holds could be stale.
let resyncHook: (() => void) | null = null
export function setResyncHook(hook: () => void) {
  resyncHook = hook
}

class WSClient {
  private ws: WebSocket | null = null
  private handlers: Set<MessageHandler> = new Set()
  private heartbeatTimer: ReturnType<typeof setInterval> | null = null
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  private url: string
  // Marks the *current* socket as intentionally closing. Reassigned on
  // every `connect()` so each socket owns its own flag via closure —
  // prevents a React StrictMode double-mount from letting the old
  // socket's `onclose` clobber the new socket's `'connected'` status.
  private markIntentional: (() => void) | null = null

  constructor(url: string) {
    this.url = url
  }

  connect() {
    if (this.ws?.readyState === WebSocket.OPEN) return

    const store = useWSStore.getState()
    store.setStatus('connecting')

    let ws: WebSocket
    try {
      ws = new WebSocket(this.url)
    } catch {
      store.setError('Failed to create WebSocket connection')
      this.scheduleReconnect()
      return
    }

    // Per-socket flag captured by the handler closures below. The old
    // socket's onclose continues to see its own `intentional` even after
    // a fresh `connect()` has installed a new one.
    let intentional = false
    this.ws = ws
    this.markIntentional = () => {
      intentional = true
    }

    ws.onopen = () => {
      const s = useWSStore.getState()
      s.setStatus('connected')
      s.resetReconnect()
      this.startHeartbeat()
      // Resync the whole scene from the server snapshot on EVERY connect.
      // The FIRST connect needs it too: a fresh page load (or refresh) must
      // hydrate existing geometry from /api/scene/snapshot — otherwise the
      // scene stays empty until some live broadcast happens to arrive, which
      // is why a refresh showed nothing. A reconnect additionally needs it
      // because the server may be a new process.
      resyncHook?.()
    }

    ws.onmessage = (event) => {
      let raw: unknown
      try {
        raw = JSON.parse(event.data)
      } catch {
        // Non-JSON frame (e.g. raw pong) — ignore. JSON parse failure
        // is the only error this `try` is guarding; schema validation
        // and dispatch live below so they can fail independently.
        return
      }
      const msg = parseServerMessage(raw)
      if (!msg) return
      for (const handler of this.handlers) {
        handler(msg)
      }
    }

    ws.onclose = () => {
      if (intentional) return
      this.stopHeartbeat()
      useWSStore.getState().setStatus('disconnected')
      this.scheduleReconnect()
    }

    ws.onerror = () => {
      if (intentional) return
      useWSStore.getState().setError('WebSocket error')
    }
  }

  disconnect() {
    this.markIntentional?.()
    this.markIntentional = null
    this.stopHeartbeat()
    this.clearReconnect()
    this.ws?.close()
    this.ws = null
    useWSStore.getState().setStatus('disconnected')
  }

  send(data: unknown) {
    if (this.ws?.readyState !== WebSocket.OPEN) return false
    this.ws.send(JSON.stringify(data))
    return true
  }

  onMessage(handler: MessageHandler) {
    this.handlers.add(handler)
    return () => {
      this.handlers.delete(handler)
    }
  }

  private startHeartbeat() {
    this.stopHeartbeat()
    this.heartbeatTimer = setInterval(() => {
      if (this.ws?.readyState === WebSocket.OPEN) {
        const start = performance.now()
        // Backend protocol: ClientMessage::Ping { timestamp: u64 }
        // Uses serde tag="type", content="data" — payload goes under `data`.
        this.ws.send(
          JSON.stringify({
            type: 'Ping',
            data: { timestamp: Math.floor(Date.now()) },
          }),
        )
        useWSStore.getState().setLastPing(Math.round(performance.now() - start))
      }
    }, HEARTBEAT_INTERVAL_MS)
  }

  private stopHeartbeat() {
    if (this.heartbeatTimer) {
      clearInterval(this.heartbeatTimer)
      this.heartbeatTimer = null
    }
  }

  private scheduleReconnect() {
    this.clearReconnect()
    const store = useWSStore.getState()
    store.incrementReconnect()
    const delay = Math.min(
      BASE_DELAY_MS * Math.pow(2, Math.min(store.reconnectAttempt, 6)),
      MAX_DELAY_MS,
    )
    this.reconnectTimer = setTimeout(() => this.connect(), delay)
  }

  private clearReconnect() {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer)
      this.reconnectTimer = null
    }
  }
}

const WS_URL = import.meta.env.VITE_WS_URL || 'ws://localhost:8081/ws'
export const wsClient = new WSClient(WS_URL)
