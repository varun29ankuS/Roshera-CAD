import { useWSStore } from '@/stores/ws-store'
import type { ServerMessage } from './protocol'

type MessageHandler = (msg: ServerMessage) => void

const MAX_RECONNECT_ATTEMPTS = 10
const BASE_DELAY_MS = 1000
const HEARTBEAT_INTERVAL_MS = 30_000

class WSClient {
  private ws: WebSocket | null = null
  private handlers: Set<MessageHandler> = new Set()
  private heartbeatTimer: ReturnType<typeof setInterval> | null = null
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  private url: string
  private intentionalClose = false

  constructor(url: string) {
    this.url = url
  }

  connect() {
    if (this.ws?.readyState === WebSocket.OPEN) return

    this.intentionalClose = false
    const store = useWSStore.getState()
    store.setStatus('connecting')

    try {
      this.ws = new WebSocket(this.url)
    } catch {
      store.setError('Failed to create WebSocket connection')
      this.scheduleReconnect()
      return
    }

    this.ws.onopen = () => {
      const s = useWSStore.getState()
      s.setStatus('connected')
      s.resetReconnect()
      this.startHeartbeat()
    }

    this.ws.onmessage = (event) => {
      try {
        const msg: ServerMessage = JSON.parse(event.data)
        for (const handler of this.handlers) {
          handler(msg)
        }
      } catch {
        // non-JSON message (pong, etc.) — ignore
      }
    }

    this.ws.onclose = () => {
      this.stopHeartbeat()
      if (!this.intentionalClose) {
        useWSStore.getState().setStatus('disconnected')
        this.scheduleReconnect()
      }
    }

    this.ws.onerror = () => {
      useWSStore.getState().setError('WebSocket error')
    }
  }

  disconnect() {
    this.intentionalClose = true
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
        this.ws.send(JSON.stringify({ type: 'Ping' }))
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
    if (store.reconnectAttempt >= MAX_RECONNECT_ATTEMPTS) {
      store.setError(`Failed to reconnect after ${MAX_RECONNECT_ATTEMPTS} attempts`)
      return
    }
    store.incrementReconnect()
    const delay = BASE_DELAY_MS * Math.pow(2, Math.min(store.reconnectAttempt, 6))
    this.reconnectTimer = setTimeout(() => this.connect(), delay)
  }

  private clearReconnect() {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer)
      this.reconnectTimer = null
    }
  }
}

const WS_URL = import.meta.env.VITE_WS_URL || 'ws://localhost:3000/ws'
export const wsClient = new WSClient(WS_URL)
