import { create } from 'zustand'
import type { AgentLook, UserContextReport } from '@/lib/user-context'

/**
 * What the app shares with the agent about the person's attention, and
 * whether an agent has read it (lib/user-context.ts; published by
 * hooks/use-user-context-publisher; shown by layout/AgentSeesChip).
 *
 * Sharing is the person's choice and is kept across reloads. Paused, the
 * server is told so once and keeps nothing else.
 */
const STORAGE_KEY = 'roshera.user-context.sharing.v1'

function storedSharing(): boolean {
  try {
    return typeof localStorage === 'undefined' || localStorage.getItem(STORAGE_KEY) !== 'off'
  } catch {
    return true
  }
}

interface UserContextState {
  sharing: boolean
  /** The last report the server acknowledged: what an agent would read. */
  acknowledged: UserContextReport | null
  /** The last agent read of this app's context. */
  look: AgentLook | null
  setSharing: (sharing: boolean) => void
  setAcknowledged: (report: UserContextReport) => void
  /** An agent read the context at `atMs` (the app's clock). An older read
   *  never replaces a newer one. */
  agentLooked: (look: AgentLook) => void
}

export const useUserContextStore = create<UserContextState>((set) => ({
  sharing: storedSharing(),
  acknowledged: null,
  look: null,
  setSharing: (sharing) => {
    try {
      localStorage.setItem(STORAGE_KEY, sharing ? 'on' : 'off')
    } catch {
      // Private mode: the choice still holds for this session.
    }
    set({ sharing })
  },
  setAcknowledged: (acknowledged) => set({ acknowledged }),
  agentLooked: (look) => set((s) => (s.look && s.look.atMs >= look.atMs ? s : { look })),
}))
