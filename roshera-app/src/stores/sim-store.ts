import { create } from 'zustand'
import type { FieldShown, LoadEntry, MaterialEntry, Setup, StaticReport, SupportEntry } from '@/lib/sim'

/**
 * The Simulate workspace: the study being set up on one part, and the last
 * report for it. The viewport reads it to mark the held and loaded faces
 * and to draw the result; the panel reads and edits it.
 */
interface SimState {
  /** Scene object id (viewport UUID) the set-up and report are about. */
  objectId: string | null
  setup: Setup
  report: StaticReport | null
  /** Which result colours the part. */
  shown: FieldShown
  /** Draw the part in its displaced shape (exaggerated). */
  deformed: boolean
  /** The set-up row the person is looking at; its faces are drawn strongest. */
  focus: { kind: 'support' | 'load'; index: number } | null
  /** Server run ids already shown or seen, so a study is loaded once. */
  seenRuns: Set<string>

  setPart: (objectId: string | null) => void
  addSupport: (s: SupportEntry) => void
  updateSupport: (index: number, s: Partial<SupportEntry>) => void
  removeSupport: (index: number) => void
  addLoad: (l: LoadEntry) => void
  updateLoad: (index: number, l: Partial<LoadEntry>) => void
  removeLoad: (index: number) => void
  setMaterial: (m: Partial<MaterialEntry>) => void
  setGoal: (minSafetyFactor: string) => void
  setMeshSize: (meshSizeMm: string) => void
  setReport: (report: StaticReport | null) => void
  show: (shown: FieldShown) => void
  setDeformed: (deformed: boolean) => void
  setFocus: (focus: SimState['focus']) => void
  /** Show a study run elsewhere (an agent): its part, set-up and report. */
  loadRun: (runId: string, objectId: string, setup: Setup, report: StaticReport) => void
  /** Remember a run as seen without showing it (the app's own run). */
  markSeen: (runId: string) => void
}

const EMPTY_SETUP: Setup = {
  supports: [],
  loads: [],
  material: { name: '', youngs_modulus_mpa: '', poisson_ratio: '', yield_strength_mpa: '', source: '' },
  minSafetyFactor: '',
  meshSizeMm: '',
}

/** Any change to the set-up makes the shown result stale: it is cleared,
 *  never left on screen beside a set-up it was not solved for. */
const edit = (_s: SimState, setup: Setup): Partial<SimState> => ({ setup, report: null })

export const useSimStore = create<SimState>((set) => ({
  objectId: null,
  setup: EMPTY_SETUP,
  report: null,
  shown: 'stress',
  deformed: true,
  focus: null,
  seenRuns: new Set(),

  setPart: (objectId) =>
    set((s) => (s.objectId === objectId ? s : { objectId, setup: EMPTY_SETUP, report: null, focus: null })),
  addSupport: (sup) => set((s) => edit(s, { ...s.setup, supports: [...s.setup.supports, sup] })),
  updateSupport: (i, sup) =>
    set((s) => edit(s, { ...s.setup, supports: s.setup.supports.map((x, j) => (j === i ? { ...x, ...sup } : x)) })),
  removeSupport: (i) =>
    set((s) => ({ ...edit(s, { ...s.setup, supports: s.setup.supports.filter((_, j) => j !== i) }), focus: null })),
  addLoad: (l) => set((s) => edit(s, { ...s.setup, loads: [...s.setup.loads, l] })),
  updateLoad: (i, l) =>
    set((s) => edit(s, { ...s.setup, loads: s.setup.loads.map((x, j) => (j === i ? { ...x, ...l } : x)) })),
  removeLoad: (i) => set((s) => ({ ...edit(s, { ...s.setup, loads: s.setup.loads.filter((_, j) => j !== i) }), focus: null })),
  setMaterial: (m) => set((s) => edit(s, { ...s.setup, material: { ...s.setup.material, ...m } })),
  setGoal: (minSafetyFactor) => set((s) => edit(s, { ...s.setup, minSafetyFactor })),
  setMeshSize: (meshSizeMm) => set((s) => edit(s, { ...s.setup, meshSizeMm })),
  setReport: (report) => set({ report }),
  show: (shown) => set({ shown }),
  setDeformed: (deformed) => set({ deformed }),
  setFocus: (focus) => set({ focus }),
  loadRun: (runId, objectId, setup, report) =>
    set((s) => ({ objectId, setup, report, focus: null, seenRuns: new Set(s.seenRuns).add(runId) })),
  markSeen: (runId) => set((s) => ({ seenRuns: new Set(s.seenRuns).add(runId) })),
}))
