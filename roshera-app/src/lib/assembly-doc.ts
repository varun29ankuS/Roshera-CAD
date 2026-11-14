/**
 * The certified assembly (`/api/assembly/...`, the kinematic-assembly
 * surface): instances of the document's parts, connectors anchored on their
 * faces, mates between connectors, and the kernel's answers -- what is still
 * free to move (DOF), whether anything touches through a motion
 * (interference), and the certificate. Not the retiring `/api/assemblies`
 * surface, whose own docs call its motion "move it and hope".
 */

const API_HOST: string =
  (import.meta as { env?: { VITE_API_URL?: string } }).env?.VITE_API_URL ?? ''

// ── Wire types ──────────────────────────────────────────────────────

export type Mat4 = number[][]

export interface InstanceSummary {
  id: string
  part_id: string
  name: string | null
  transform: Mat4
  resolved_solid: number | null
  /** The part's soundness; null when its part cannot be resolved. */
  sound: boolean | null
}

/** A mate kind as the kernel writes it: a unit kind is a string, a kind
 *  with settings an object keyed by the kind. */
export type MateKind = string | Record<string, unknown>

export interface MateListing {
  id: string
  kind: MateKind
  a_instance: string
  b_instance: string
  a_connector: string
  b_connector: string
}

export interface AssemblySummary {
  id: string
  name: string
  instance_count: number
  unique_part_count: number
  instances: InstanceSummary[]
  bbox: [number[], number[]] | null
  all_sound: boolean
  /** Present on servers that list mates on the read. */
  mates?: MateListing[]
}

export type Motion =
  | { motion: 'rotation_about'; point: number[]; axis: number[] }
  | { motion: 'translation_along'; direction: number[] }
  | { motion: 'screw_about'; point: number[]; axis: number[]; pitch: number }

export type Constrainment =
  | { status: 'fully_constrained' }
  | { status: 'mobile'; dof: number; motions: Motion[] }
  | { status: 'over_constrained'; via: string[] }

export type AssemblyConstrainedness =
  | { status: 'fully_constrained' }
  | { status: 'mobile'; dof: number }
  | { status: 'over_constrained'; redundant: number }
  | { status: 'conflicting'; conflicts: number }

export interface DofReport {
  computed: boolean
  refused_reason: string | null
  constrainedness: AssemblyConstrainedness | null
  instances: { instance_id: string; constrainment: Constrainment }[]
}

export interface InterferenceSweep {
  range: [number, number]
  clear: boolean
  min_certified_clearance: number | null
  epsilon: number
  first_contact: unknown
  refusal: unknown
}

export interface InterferenceReport {
  computed: boolean
  refused_reason: string | null
  static_pairs: unknown[]
  sweeps: InterferenceSweep[]
}

// ── Readings ────────────────────────────────────────────────────────

export type Tone = 'positive' | 'info' | 'caution' | 'negative' | 'muted'

const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? '' : 's'}`

/** The whole assembly's verdict, as one short line. */
export function assemblyStatus(c: AssemblyConstrainedness | null): { tone: Tone; text: string } {
  if (!c) return { tone: 'muted', text: 'not solved' }
  switch (c.status) {
    case 'fully_constrained':
      return { tone: 'positive', text: '✓ fully located' }
    case 'mobile':
      return { tone: 'info', text: `${plural(c.dof, 'free motion')}` }
    case 'over_constrained':
      return { tone: 'caution', text: `${plural(c.redundant, 'redundant mate')}` }
    case 'conflicting':
      return { tone: 'negative', text: `✗ ${plural(c.conflicts, 'conflict')}` }
  }
}

/** A direction as a word when it is an axis, else its rounded vector. */
function axisWord(v: number[]): string {
  const names = ['X', 'Y', 'Z']
  const i = v.findIndex((x) => Math.abs(Math.abs(x) - 1) < 1e-6)
  if (i >= 0 && v.every((x, k) => k === i || Math.abs(x) < 1e-6)) return names[i]
  return `(${v.map((x) => Number(x.toFixed(2))).join(', ')})`
}

/** What one instance may still do: grounded, fixed, or its motions. */
export function freedomLabel(c: Constrainment, grounded: boolean): string {
  if (c.status === 'fully_constrained') return grounded ? 'grounded' : 'fixed'
  if (c.status === 'over_constrained') return 'over-constrained'
  if (c.motions.length === 0) return `free (${c.dof})`
  return c.motions
    .map((m) =>
      m.motion === 'rotation_about'
        ? `rotates about ${axisWord(m.axis)}`
        : m.motion === 'translation_along'
          ? `slides along ${axisWord(m.direction)}`
          : `screws about ${axisWord(m.axis)}`,
    )
    .join(' · ')
}

/** A mate kind's name. */
export function mateKindName(k: MateKind): string {
  return typeof k === 'string' ? k : (Object.keys(k)[0] ?? 'Mate')
}

const GLYPHS: Record<string, string> = {
  Fastened: '⚓',
  Revolute: '⟳',
  Slider: '↔',
  Cylindrical: '⇅',
  Planar: '▭',
  Ball: '◉',
  PinSlot: '⊸',
  Distance: '↦',
  Angle: '∠',
  Parallel: '∥',
  Tangent: '◠',
  GearRatio: '⚙',
  RackPinion: '⚙',
  Screw: '⟲',
}

/** A mate kind's glyph (the row icon). */
export function mateGlyph(k: MateKind): string {
  return GLYPHS[mateKindName(k)] ?? '◇'
}

// ── Calls ───────────────────────────────────────────────────────────

async function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const r = await fetch(`${API_HOST}${path}`, {
    method,
    headers: body === undefined ? undefined : { 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  if (!r.ok) {
    const err = (await r.json().catch(() => ({}))) as { error?: string; message?: string }
    throw new Error(err.error ?? err.message ?? `HTTP ${r.status}`)
  }
  return (await r.json()) as T
}

export const listAssemblies = () => call<string[]>('GET', '/api/assembly')
export const getAssembly = (id: string) => call<AssemblySummary>('GET', `/api/assembly/${id}`)
export const createAssembly = (name: string) => call<{ id: string }>('POST', '/api/assembly', { name })
export const deleteAssembly = (id: string) => call<unknown>('DELETE', `/api/assembly/${id}`)
export const addInstance = (id: string, partId: string, name: string) =>
  call<{ instance_id: string }>('POST', `/api/assembly/${id}/instance`, { part_id: partId, name })
export const removeInstance = (id: string, instanceId: string) =>
  call<unknown>('DELETE', `/api/assembly/${id}/instance/${instanceId}`)
export const addFaceConnector = (id: string, instanceId: string, faceId: number) =>
  call<{ connector_id: string; provenance: string }>('POST', `/api/assembly/${id}/connector`, {
    instance_id: instanceId,
    face: { face_id: faceId },
  })
export const addMate = (id: string, kind: MateKind, a: string, b: string) =>
  call<{ mate_id: string }>('POST', `/api/assembly/${id}/mate`, { kind, a, b })
export const removeMate = (id: string, mateId: string) => call<unknown>('DELETE', `/api/assembly/${id}/mate/${mateId}`)
export const solveAssembly = (id: string) =>
  call<{ solved: boolean; refused_reason: string | null; poses: { instance_id: string; transform: Mat4 }[] }>(
    'POST',
    `/api/assembly/${id}/solve`,
    {},
  )
export const getDof = (id: string) => call<DofReport>('GET', `/api/assembly/${id}/dof`)
export const getInterference = (id: string) => call<InterferenceReport>('GET', `/api/assembly/${id}/interference`)
