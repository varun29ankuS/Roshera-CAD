import { create } from 'zustand'
import {
  getAssembly,
  getDof,
  getInterference,
  listAssemblies,
  type AssemblySummary,
  type DofReport,
  type InterferenceReport,
} from '@/lib/assembly-doc'

/**
 * The open certified assembly (lib/assembly-doc): its summary, and the
 * kernel's DOF and interference answers for it. The viewport draws its
 * instances from here; the Assembly panel reads and edits it.
 */
interface AssemblyDocState {
  ids: string[]
  names: Record<string, string>
  activeId: string | null
  summary: AssemblySummary | null
  dof: DofReport | null
  interference: InterferenceReport | null
  /** The interference sweep is the slow read: true while it runs. */
  checking: boolean
  error: string | null
  refreshList: () => Promise<void>
  open: (id: string | null) => Promise<void>
  /** Re-read the open assembly (after an edit), DOF included. */
  reload: () => Promise<void>
  /** Run the clearance check through the free motions. */
  check: () => Promise<void>
  setError: (e: string | null) => void
}

const message = (e: unknown) => (e instanceof Error ? e.message : String(e))

export const useAssemblyDocStore = create<AssemblyDocState>((set, get) => ({
  ids: [],
  names: {},
  activeId: null,
  summary: null,
  dof: null,
  interference: null,
  checking: false,
  error: null,
  refreshList: async () => {
    try {
      const ids = await listAssemblies()
      const names: Record<string, string> = {}
      await Promise.all(
        ids.map(async (id) => {
          names[id] = (await getAssembly(id).catch(() => null))?.name ?? 'Assembly'
        }),
      )
      set({ ids, names })
    } catch (e) {
      set({ error: message(e) })
    }
  },
  open: async (id) => {
    set({ activeId: id, summary: null, dof: null, interference: null, error: null })
    if (id) await get().reload()
  },
  reload: async () => {
    const id = get().activeId
    if (!id) return
    try {
      const [summary, dof] = await Promise.all([getAssembly(id), getDof(id).catch(() => null)])
      if (get().activeId !== id) return
      set({ summary, dof, error: null })
    } catch (e) {
      set({ error: message(e) })
    }
  },
  check: async () => {
    const id = get().activeId
    if (!id) return
    set({ checking: true })
    try {
      const interference = await getInterference(id)
      if (get().activeId === id) set({ interference })
    } catch (e) {
      set({ error: message(e) })
    } finally {
      set({ checking: false })
    }
  },
  setError: (error) => set({ error }),
}))
