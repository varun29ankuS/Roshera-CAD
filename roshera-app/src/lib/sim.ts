/**
 * Simulation: set up a linear static study on a part, run it on the kernel
 * (`POST /api/agent/parts/{id}/sim/static`, `sim_engine::run::StaticReport`)
 * and read the answer in an engineer's words.
 *
 * The verdict is the kernel's: holds / fails against the goal, or
 * undecided WITH ITS REASON. An undecided study is never shown as a pass,
 * and no safety factor is shown that the kernel did not give.
 */

const API_HOST: string =
  (import.meta as { env?: { VITE_API_URL?: string } }).env?.VITE_API_URL ?? ''

export type Vec3 = [number, number, number]

// ─── The kernel's types (sim-engine, serde shapes) ──────────────────────

export type SupportKind = 'fixed' | 'bolt_clamp' | 'pin' | 'slider'
export type Spread = 'even' | 'bearing'

export interface Study {
  supports: { kind: SupportKind; faces: number[] }[]
  loads: { faces: number[]; force_n: Vec3; spread: Spread }[]
  material: {
    name: string
    youngs_modulus_mpa: number
    poisson_ratio: number
    yield_strength_mpa?: number
    source?: string
  }
  goal: { min_safety_factor: number }
  mesh: { size_mm?: number }
}

export interface MeshStats {
  size_mm: number
  nodes: number
  elements: number
  boundary_deviation_mm: number
  mesher_seconds: number
}

export interface RunResult {
  mesh: MeshStats
  max_displacement_mm: number
  peak_von_mises_mpa: number
  peak_at: Vec3
  reaction_total_n: Vec3
  load_on_held_n: Vec3
  solver_seconds: number
}

export interface SurfaceField {
  positions: Vec3[]
  displacement: Vec3[]
  von_mises: number[]
  triangles: Vec3[]
  faces: number[]
  min_von_mises: number
  max_von_mises: number
}

export type Verdict =
  | { verdict: 'holds'; safety_factor: number; required: number }
  | { verdict: 'fails'; safety_factor: number; required: number }
  | { verdict: 'undecided'; reason: string }

export interface EngineInfo {
  name: string
  program: string
  version: string
  interim: boolean
}

export interface StaticReport {
  verdict: Verdict
  checks: { name: string; passed: boolean; detail: string }[]
  coarse: RunResult
  fine: RunResult
  engines: { mesher: EngineInfo; solver: EngineInfo }
  face_pids: [number, string][]
  assumptions: string[]
  field: SurfaceField
}

// ─── The set-up as the person enters it ─────────────────────────────────

/** Directions a load can be given in: along a model axis, or pushing into
 *  the loaded face (against its outward normal). */
export type LoadDirection = '+x' | '-x' | '+y' | '-y' | '+z' | '-z' | 'into'

export const LOAD_DIRECTIONS: { id: LoadDirection; label: string }[] = [
  { id: 'into', label: 'Into the face' },
  { id: '-z', label: '−Z (down)' },
  { id: '+z', label: '+Z (up)' },
  { id: '+x', label: '+X' },
  { id: '-x', label: '−X' },
  { id: '+y', label: '+Y' },
  { id: '-y', label: '−Y' },
]

export const SUPPORT_KINDS: { id: SupportKind; label: string; hint: string; solved: boolean }[] = [
  { id: 'fixed', label: 'Fixed', hint: 'Held in every direction: welded, bonded, clamped hard', solved: true },
  { id: 'bolt_clamp', label: 'Bolted', hint: 'A bolt clamps these faces (washer face, hole wall) to a rigid frame', solved: true },
  { id: 'pin', label: 'Pin', hint: 'A pin in a hole: held across, free to turn. Checked for motion; not solved yet', solved: false },
  { id: 'slider', label: 'Slider', hint: 'Rests on a surface: held along its normal only. Checked for motion; not solved yet', solved: false },
]

export interface SupportEntry {
  kind: SupportKind
  faces: number[]
}

export interface LoadEntry {
  faces: number[]
  direction: LoadDirection
  /** Total force, N (positive). */
  newtons: number
  spread: Spread
  /** The loaded faces' outward normal (area-weighted, model axes), needed
   *  for an 'into' load. */
  normal: Vec3 | null
}

export interface MaterialEntry {
  name: string
  youngs_modulus_mpa: string
  poisson_ratio: string
  yield_strength_mpa: string
  source: string
}

export interface Setup {
  supports: SupportEntry[]
  loads: LoadEntry[]
  material: MaterialEntry
  minSafetyFactor: string
  meshSizeMm: string
}

const AXES: Record<Exclude<LoadDirection, 'into'>, Vec3> = {
  '+x': [1, 0, 0],
  '-x': [-1, 0, 0],
  '+y': [0, 1, 0],
  '-y': [0, -1, 0],
  '+z': [0, 0, 1],
  '-z': [0, 0, -1],
}

/** A load's total force vector, N, or null when it has no direction. */
export function forceOf(load: LoadEntry): Vec3 | null {
  let d: Vec3
  if (load.direction === 'into') {
    const n = load.normal
    const len = n ? Math.hypot(n[0], n[1], n[2]) : 0
    if (!n || !(len > 0)) return null
    d = [-n[0] / len, -n[1] / len, -n[2] / len]
  } else {
    d = AXES[load.direction]
  }
  const f = load.newtons
  return [d[0] * f, d[1] * f, d[2] * f]
}

const num = (s: string): number | null => {
  if (s.trim() === '') return null
  const n = Number(s)
  return Number.isFinite(n) ? n : null
}

/** Why the set-up cannot be run yet, step by step (empty: ready). */
export function setupGaps(setup: Setup): string[] {
  const gaps: string[] = []
  if (setup.supports.length === 0 || setup.supports.every((s) => s.faces.length === 0))
    gaps.push('Say how the part is held: pick the faces that are bolted or fixed')
  if (setup.loads.length === 0) gaps.push('Add a load: pick the faces it acts on')
  for (const l of setup.loads) {
    if (!(l.newtons > 0)) gaps.push('Every load needs a force above 0 N')
    else if (forceOf(l) === null) gaps.push('A load pushing into its faces needs faces with one clear normal')
  }
  const m = setup.material
  const e = num(m.youngs_modulus_mpa)
  const nu = num(m.poisson_ratio)
  if (m.name.trim() === '') gaps.push('Name the material')
  if (e === null || e <= 0) gaps.push("Give the material's Young's modulus (MPa)")
  if (nu === null || nu <= 0 || nu >= 0.5) gaps.push("Give the material's Poisson ratio (between 0 and 0.5)")
  const y = num(m.yield_strength_mpa)
  if (m.yield_strength_mpa.trim() !== '' && (y === null || y <= 0)) gaps.push('The yield strength must be above 0 MPa')
  const sf = num(setup.minSafetyFactor)
  if (sf === null || sf <= 0) gaps.push('Set the safety factor the part must reach')
  const size = num(setup.meshSizeMm)
  if (setup.meshSizeMm.trim() !== '' && (size === null || size <= 0)) gaps.push('The mesh size must be above 0 mm')
  return [...new Set(gaps)]
}

/** The kernel's `Study` for a complete set-up, or null when it has gaps. */
export function studyOf(setup: Setup): Study | null {
  if (setupGaps(setup).length > 0) return null
  const m = setup.material
  const y = num(m.yield_strength_mpa)
  const size = num(setup.meshSizeMm)
  return {
    supports: setup.supports.filter((s) => s.faces.length > 0).map((s) => ({ kind: s.kind, faces: [...s.faces] })),
    loads: setup.loads.map((l) => ({ faces: [...l.faces], force_n: forceOf(l) as Vec3, spread: l.spread })),
    material: {
      name: m.name.trim(),
      youngs_modulus_mpa: Number(m.youngs_modulus_mpa),
      poisson_ratio: Number(m.poisson_ratio),
      ...(y !== null ? { yield_strength_mpa: y } : {}),
      ...(m.source.trim() !== '' ? { source: m.source.trim() } : {}),
    },
    goal: { min_safety_factor: Number(setup.minSafetyFactor) },
    mesh: size !== null ? { size_mm: size } : {},
  }
}

// ─── Running it ─────────────────────────────────────────────────────────

export type SimRun =
  | { kind: 'report'; report: StaticReport }
  /** Changed since it was last verified: verify, then run again. */
  | { kind: 'unverified'; reason: string }
  /** The supports leave the part free to move; the motions in words. */
  | { kind: 'can_move'; reason: string; motions: string[] }
  /** The mesher or solver is not installed on the server. */
  | { kind: 'engine_missing'; reason: string }
  | { kind: 'refused'; reason: string }

/** The route's answer as a `SimRun` (pure; `runStatic` does the fetch). */
export function simRunOf(status: number, body: Record<string, unknown>): SimRun {
  if (status >= 200 && status < 300) return { kind: 'report', report: body as unknown as StaticReport }
  const reason = typeof body.reason === 'string' ? body.reason : `HTTP ${status}`
  switch (body.error) {
    case 'sim_stale_solid':
      return { kind: 'unverified', reason }
    case 'sim_part_can_move':
      return {
        kind: 'can_move',
        reason,
        motions: Array.isArray(body.free_motions) ? body.free_motions.filter((m): m is string => typeof m === 'string') : [],
      }
    case 'sim_engine_missing':
      return { kind: 'engine_missing', reason }
    default:
      return { kind: 'refused', reason }
  }
}

/** Run a static study on a part (by its kernel solid id). */
export async function runStatic(solidId: number, study: Study): Promise<SimRun> {
  const r = await fetch(`${API_HOST}/api/agent/parts/${solidId}/sim/static`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(study),
  })
  const body = (await r.json().catch(() => ({}))) as Record<string, unknown>
  return simRunOf(r.status, body)
}

// ─── Reading the answer ─────────────────────────────────────────────────

export type SimState = 'pass' | 'fix' | 'unknown'

/** The verdict as a headline and a line under it. */
export function verdictView(v: Verdict): { state: SimState; title: string; detail: string } {
  switch (v.verdict) {
    case 'holds':
      return {
        state: 'pass',
        title: `Holds — safety factor ${v.safety_factor.toFixed(1)}`,
        detail: `The goal was ${v.required.toFixed(1)}.`,
      }
    case 'fails':
      return {
        state: 'fix',
        title: `Fails — safety factor ${v.safety_factor.toFixed(2)}`,
        detail: `The goal is ${v.required.toFixed(1)}.`,
      }
    case 'undecided':
      return { state: 'unknown', title: 'No verdict', detail: v.reason }
  }
}

/** Stress colours: blue (low) through cyan, green, yellow to red (high) --
 *  the scale engineers read results in. `t` in [0, 1]. */
export function stressColor(t: number): Vec3 {
  const x = Math.min(1, Math.max(0, Number.isFinite(t) ? t : 0))
  const stops: Vec3[] = [
    [0.13, 0.27, 0.8],
    [0.1, 0.7, 0.85],
    [0.2, 0.75, 0.3],
    [0.95, 0.8, 0.15],
    [0.85, 0.15, 0.12],
  ]
  const s = x * (stops.length - 1)
  const i = Math.min(stops.length - 2, Math.floor(s))
  const f = s - i
  const a = stops[i]
  const b = stops[i + 1]
  return [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f]
}

export type FieldShown = 'stress' | 'displacement'

const mag = (d: Vec3) => Math.hypot(d[0], d[1], d[2])

/** The value range a legend shows for a field. */
export function fieldRange(field: SurfaceField, shown: FieldShown): { min: number; max: number; unit: string } {
  if (shown === 'stress') return { min: field.min_von_mises, max: field.max_von_mises, unit: 'MPa' }
  let max = 0
  for (const d of field.displacement) max = Math.max(max, mag(d))
  return { min: 0, max, unit: 'mm' }
}

/** How much to exaggerate the displacement so the shape change can be seen:
 *  the largest movement drawn as 5% of the part's size. 1 when it already
 *  shows, or when nothing moved. */
export function deformScale(field: SurfaceField): number {
  if (field.positions.length === 0) return 1
  const lo: Vec3 = [Infinity, Infinity, Infinity]
  const hi: Vec3 = [-Infinity, -Infinity, -Infinity]
  for (const p of field.positions)
    for (let k = 0; k < 3; k++) {
      lo[k] = Math.min(lo[k], p[k])
      hi[k] = Math.max(hi[k], p[k])
    }
  const size = Math.max(hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2])
  let max = 0
  for (const d of field.displacement) max = Math.max(max, mag(d))
  if (!(max > 0) || !(size > 0)) return 1
  const k = (0.05 * size) / max
  if (k <= 1) return 1
  // A round number, so the label reads like a scale someone chose.
  const p = 10 ** Math.floor(Math.log10(k))
  return [1, 2, 5, 10].map((m) => m * p).filter((v) => v <= k).pop() ?? p
}

/** Vertex buffers for the result mesh: positions (moved by `scale` times
 *  the displacement), per-vertex colours for the shown field, and the
 *  triangle index. Model coordinates. */
export function fieldBuffers(
  field: SurfaceField,
  shown: FieldShown,
  scale: number,
): { positions: Float32Array; colors: Float32Array; index: Uint32Array } {
  const n = field.positions.length
  const positions = new Float32Array(n * 3)
  const colors = new Float32Array(n * 3)
  const { min, max } = fieldRange(field, shown)
  const span = max - min
  for (let i = 0; i < n; i++) {
    const p = field.positions[i]
    const d = field.displacement[i] ?? [0, 0, 0]
    for (let k = 0; k < 3; k++) positions[i * 3 + k] = p[k] + scale * d[k]
    const v = shown === 'stress' ? (field.von_mises[i] ?? 0) : mag(d)
    const c = stressColor(span > 0 ? (v - min) / span : 0)
    colors.set(c, i * 3)
  }
  const index = new Uint32Array(field.triangles.length * 3)
  field.triangles.forEach((t, j) => index.set(t, j * 3))
  return { positions, colors, index }
}

/** A short engine label, interim engines named as such. */
export function engineLabel(e: EngineInfo): string {
  const v = e.version.trim() ? ` ${e.version.trim()}` : ''
  return `${e.name}${v}${e.interim ? ' (interim)' : ''}`
}
