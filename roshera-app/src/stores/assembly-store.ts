/**
 * Assembly-workspace store. Holds the currently-active assembly id and
 * its last-fetched snapshot so multiple panels (outliner, mate editor,
 * inspector) can share state without independently re-fetching.
 *
 * The store does NOT subscribe to WebSocket events — for the demo the
 * mate-editor flow is fully synchronous via REST. Snapshot refresh is
 * imperative: callers (`refresh()`, or any handler that just mutated)
 * trigger it explicitly.
 */

import { create } from 'zustand'
import {
  getAssembly,
  listAssemblies,
  createAssembly,
  type AssemblySummary,
} from '@/lib/assembly-api'

/**
 * One leg of an in-progress mate pick. Captured at click time on a
 * face of an `asm-comp:*` mesh.
 *
 * `origin` and `normal` are in the component's *local* frame so the
 * kernel's `MateReference::Plane` carries through correctly when the
 * solver later applies the component transform. The pick `point` /
 * `screen` pair is kept in world / pixel space so the popover that
 * confirms the mate can be anchored to where the user clicked.
 */
export interface PendingPick {
  componentId: string
  /** Pick origin in component-local coordinates (mm). */
  origin: [number, number, number]
  /** Surface normal at the pick in component-local coordinates (unit). */
  normal: [number, number, number]
  /** World-space pick point — for popover anchoring. */
  worldPoint: [number, number, number]
  /** Screen-space pick position (px) at click time. */
  screen: { x: number; y: number }
}

export interface PendingMate {
  ref1: PendingPick | null
  ref2: PendingPick | null
}

interface AssemblyState {
  /** All known assembly ids (UUIDs); refreshed by `refreshList()`. */
  ids: string[]
  /** Currently-active assembly id, or `null` if none selected. */
  activeId: string | null
  /** Snapshot of the active assembly (components + mates). */
  active: AssemblySummary | null
  /** True while a fetch is in flight; used to gate redundant calls. */
  loading: boolean
  /** Last error string, or `null` if the last op succeeded. */
  error: string | null
  /** Mate-pick flow state. `ref1`/`ref2` populate sequentially. */
  pendingMate: PendingMate

  /** Re-fetch the id list from the server. */
  refreshList: () => Promise<void>
  /** Re-fetch the active assembly's snapshot. No-op if `activeId === null`. */
  refreshActive: () => Promise<void>
  /** Set the active id and immediately fetch its snapshot. */
  setActive: (id: string | null) => Promise<void>
  /** Create an assembly and switch the active id to it. Returns the new id. */
  createAndActivate: (name: string) => Promise<string>
  /** Apply a snapshot directly (used after mate add / patch / etc.). */
  setSnapshot: (snap: AssemblySummary) => void
  /** Set the error banner. */
  setError: (e: string | null) => void
  /**
   * Push a pick into the pending mate. Slot 1 fills first; the next
   * pick on a *different* component fills slot 2. A pick on the same
   * component as slot 1 replaces slot 1 so users can correct
   * themselves without having to cancel.
   */
  addPendingPick: (pick: PendingPick) => void
  /** Reset the mate-pick flow (Esc / cancel / assembly switch). */
  clearPendingMate: () => void
}

export const useAssemblyStore = create<AssemblyState>((set, get) => ({
  ids: [],
  activeId: null,
  active: null,
  loading: false,
  error: null,
  pendingMate: { ref1: null, ref2: null },

  refreshList: async () => {
    try {
      const ids = await listAssemblies()
      set({ ids })
    } catch (e) {
      set({ error: e instanceof Error ? e.message : String(e) })
    }
  },

  refreshActive: async () => {
    const id = get().activeId
    if (!id) return
    set({ loading: true })
    try {
      const snap = await getAssembly(id)
      set({ active: snap, loading: false })
    } catch (e) {
      set({
        error: e instanceof Error ? e.message : String(e),
        loading: false,
      })
    }
  },

  setActive: async (id) => {
    // Switching assemblies invalidates any in-flight mate picks — the
    // captured component ids reference the previous snapshot.
    set({ activeId: id, active: null, pendingMate: { ref1: null, ref2: null } })
    if (id) {
      await get().refreshActive()
    }
  },

  createAndActivate: async (name) => {
    const id = await createAssembly(name)
    await get().refreshList()
    await get().setActive(id)
    return id
  },

  setSnapshot: (snap) => {
    set({ active: snap, activeId: snap.id })
  },

  setError: (e) => set({ error: e }),

  addPendingPick: (pick) => {
    const cur = get().pendingMate
    if (cur.ref1 === null) {
      set({ pendingMate: { ref1: pick, ref2: null } })
      return
    }
    // Same component re-pick → replace slot 1 instead of attempting a
    // mate against the same component (the kernel rejects those and a
    // self-mate is never the intent).
    if (cur.ref1.componentId === pick.componentId) {
      set({ pendingMate: { ref1: pick, ref2: null } })
      return
    }
    set({ pendingMate: { ref1: cur.ref1, ref2: pick } })
  },

  clearPendingMate: () => set({ pendingMate: { ref1: null, ref2: null } }),
}))
