import { create } from 'zustand'
import {
  setupOf,
  stepOfStage,
  type StepId,
  type FieldShown,
  type LoadEntry,
  type LocalEntry,
  type MaterialEntry,
  type MeshPreview,
  type MeshStats,
  type MeshWire,
  type Setup,
  type SimSession,
  type SimStage,
  type StaticReport,
  type SupportEntry,
} from '@/lib/sim'

/**
 * The Simulate workspace: the study on one part, at its stage -- the mesh,
 * the set-up, or the result -- and what that stage shows. The study is the
 * document's session on the server (`GET /api/agent/sim/session`), whoever
 * drives it: an agent walking the person through it, or the person in the
 * panel. The viewport reads this to draw the mesh, the held and loaded
 * faces, or the result; the panel reads and edits it.
 */
interface SimState {
  /** Scene object id (viewport UUID) the study is about. */
  objectId: string | null
  /** Where the study is: null = being set up in the panel, not yet staged. */
  stage: SimStage | null
  /** The server revision on screen; a new one is loaded once. */
  revision: string | null
  mesh: MeshPreview | null
  /** The last proving mesh made, kept after a settings change clears the
   *  mesh -- the node estimate for new settings scales from it. */
  lastMeshFine: MeshStats | null
  wire: MeshWire | null
  setup: Setup
  report: StaticReport | null
  /** Which result colours the part. */
  shown: FieldShown
  /** Draw the part in its displaced shape (exaggerated). */
  deformed: boolean
  /** The set-up row the person is looking at; its faces are drawn strongest. */
  focus: { kind: 'support' | 'load'; index: number } | null
  /** The step on screen. */
  step: StepId
  /** The support, load or finer-here row whose faces the person is clicking now. */
  picking: { kind: 'support' | 'load' | 'local'; index: number } | null

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
  /** A mesh setting changed: the mesh on screen no longer matches it and is
   *  cleared, to be made again. */
  setMesh: (patch: Partial<Pick<Setup, 'meshSizeMm' | 'curveElements' | 'refineAtPeak' | 'maxRefinements' | 'nodeBudget'>>) => void
  setAccuracy: (patch: Partial<Pick<Setup, 'stressPct' | 'displacementPct'>>) => void
  addLocal: (l: LocalEntry) => void
  updateLocal: (index: number, l: Partial<LocalEntry>) => void
  removeLocal: (index: number) => void
  setReport: (report: StaticReport | null) => void
  show: (shown: FieldShown) => void
  setDeformed: (deformed: boolean) => void
  setFocus: (focus: SimState['focus']) => void
  /** Show the document's study at its stage, on its part. */
  loadSession: (objectId: string, session: SimSession) => void
  /** Note a revision as already on screen (the panel's own run). */
  markSeen: (revision: string) => void
  setStep: (step: StepId) => void
  setPicking: (picking: SimState['picking']) => void
}

const EMPTY_SETUP: Setup = {
  supports: [],
  loads: [],
  material: { name: '', youngs_modulus_mpa: '', poisson_ratio: '', yield_strength_mpa: '', source: '', yieldBasis: null },
  minSafetyFactor: '',
  meshSizeMm: '',
  curveElements: '',
  local: [],
  refineAtPeak: true,
  maxRefinements: '',
  nodeBudget: '',
  stressPct: '',
  displacementPct: '',
}

/** Any change to the set-up in the panel makes what is shown stale: the
 *  result is cleared and the study leaves its staged flow (never a result
 *  left on screen beside a set-up it was not solved for). */
const edit = (setup: Setup): Partial<SimState> => ({ setup, report: null, stage: null })

export const useSimStore = create<SimState>((set) => ({
  objectId: null,
  stage: null,
  revision: null,
  mesh: null,
  lastMeshFine: null,
  wire: null,
  setup: EMPTY_SETUP,
  report: null,
  shown: 'stress',
  deformed: true,
  focus: null,
  step: 'part',
  picking: null,

  setPart: (objectId) =>
    set((s) =>
      s.objectId === objectId
        ? s
        : { objectId, setup: EMPTY_SETUP, report: null, stage: null, mesh: null, lastMeshFine: s.mesh?.fine ?? s.lastMeshFine, wire: null, focus: null, picking: null },
    ),
  addSupport: (sup) => set((s) => edit({ ...s.setup, supports: [...s.setup.supports, sup] })),
  updateSupport: (i, sup) =>
    set((s) => edit({ ...s.setup, supports: s.setup.supports.map((x, j) => (j === i ? { ...x, ...sup } : x)) })),
  removeSupport: (i) =>
    set((s) => ({ ...edit({ ...s.setup, supports: s.setup.supports.filter((_, j) => j !== i) }), focus: null })),
  addLoad: (l) => set((s) => edit({ ...s.setup, loads: [...s.setup.loads, l] })),
  updateLoad: (i, l) =>
    set((s) => edit({ ...s.setup, loads: s.setup.loads.map((x, j) => (j === i ? { ...x, ...l } : x)) })),
  removeLoad: (i) => set((s) => ({ ...edit({ ...s.setup, loads: s.setup.loads.filter((_, j) => j !== i) }), focus: null })),
  setMaterial: (m) => set((s) => edit({ ...s.setup, material: { ...s.setup.material, ...m } })),
  setGoal: (minSafetyFactor) => set((s) => edit({ ...s.setup, minSafetyFactor })),
  setMeshSize: (meshSizeMm) => set((s) => ({ ...edit({ ...s.setup, meshSizeMm }), mesh: null, lastMeshFine: s.mesh?.fine ?? s.lastMeshFine, wire: null })),
  setMesh: (patch) => set((s) => ({ ...edit({ ...s.setup, ...patch }), mesh: null, lastMeshFine: s.mesh?.fine ?? s.lastMeshFine, wire: null })),
  setAccuracy: (patch) => set((s) => edit({ ...s.setup, ...patch })),
  addLocal: (l) => set((s) => ({ ...edit({ ...s.setup, local: [...s.setup.local, l] }), mesh: null, lastMeshFine: s.mesh?.fine ?? s.lastMeshFine, wire: null })),
  updateLocal: (i, l) =>
    set((s) => ({
      ...edit({ ...s.setup, local: s.setup.local.map((x, j) => (j === i ? { ...x, ...l } : x)) }),
      mesh: null,
      lastMeshFine: s.mesh?.fine ?? s.lastMeshFine,
      wire: null,
    })),
  removeLocal: (i) =>
    set((s) => ({ ...edit({ ...s.setup, local: s.setup.local.filter((_, j) => j !== i) }), mesh: null, lastMeshFine: s.mesh?.fine ?? s.lastMeshFine, wire: null, focus: null })),
  setReport: (report) => set({ report, stage: report ? 'result' : null }),
  show: (shown) => set({ shown }),
  setDeformed: (deformed) => set({ deformed }),
  setFocus: (focus) => set({ focus }),
  loadSession: (objectId, session) =>
    set({
      objectId,
      stage: session.stage,
      revision: session.revision,
      mesh: session.mesh,
      lastMeshFine: session.mesh?.fine ?? null,
      wire: session.wire,
      setup: session.study ? setupOf(session.study) : EMPTY_SETUP,
      report: session.stage === 'result' ? session.report : null,
      focus: null,
      picking: null,
      step: stepOfStage(session.stage),
    }),
  markSeen: (revision) => set({ revision }),
  setStep: (step) => set({ step, picking: null }),
  setPicking: (picking) => set({ picking }),
}))
