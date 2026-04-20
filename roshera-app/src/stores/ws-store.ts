import { create } from 'zustand'

export type ConnectionStatus = 'disconnected' | 'connecting' | 'connected' | 'error'

interface WSState {
  status: ConnectionStatus
  sessionId: string | null
  error: string | null
  lastPingMs: number | null
  reconnectAttempt: number

  setStatus: (status: ConnectionStatus) => void
  setSessionId: (id: string | null) => void
  setError: (error: string | null) => void
  setLastPing: (ms: number) => void
  incrementReconnect: () => void
  resetReconnect: () => void
}

export const useWSStore = create<WSState>((set) => ({
  status: 'disconnected',
  sessionId: null,
  error: null,
  lastPingMs: null,
  reconnectAttempt: 0,

  setStatus: (status) => set({ status }),
  setSessionId: (id) => set({ sessionId: id }),
  setError: (error) => set({ error, status: 'error' }),
  setLastPing: (ms) => set({ lastPingMs: ms }),
  incrementReconnect: () =>
    set((state) => ({ reconnectAttempt: state.reconnectAttempt + 1 })),
  resetReconnect: () => set({ reconnectAttempt: 0 }),
}))
