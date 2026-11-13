import { create } from 'zustand'

/**
 * What the agent's UI shares between the AI button (which owns the setup
 * dialog) and the Blackboard (the agent's window, whose header chip names
 * the configured AI and opens that setup).
 */
interface AgentUiState {
  /** The provider and model saved on the server, or null when none is.
   *  Known before any agent session runs, so the chip can name it. */
  configured: { provider: string; model: string | null } | null
  /** Bumped by `openSettings`; the dialog opens on every change. */
  settingsRequests: number
  setConfigured: (c: { provider: string; model: string | null } | null) => void
  openSettings: () => void
}

export const useAgentUiStore = create<AgentUiState>((set) => ({
  configured: null,
  settingsRequests: 0,
  setConfigured: (configured) => set({ configured }),
  openSettings: () => set((s) => ({ settingsRequests: s.settingsRequests + 1 })),
}))
