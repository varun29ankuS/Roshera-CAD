// Kernel Lens — what the kernel did to a part, face by face, and why.
//
// The kernel records every decision it makes on import (a loop walked
// reversed, an edge gap repaired, a face reversed against its neighbours…)
// and every certificate check a face blocks, as events keyed by the face's
// persistent id. This module turns the server's journal into what the canvas
// and the story card show. The agent reads the same journal over MCP.

export type LensStatus = 'as_written' | 'repaired' | 'blocking'

export type LensKind =
  | 'loop_reversed'
  | 'outer_loop_chosen'
  | 'profile_rescaled'
  | 'edge_tolerance_raised'
  | 'loop_reversed_to_match_neighbours'
  | 'void_shell_reversed'
  | { blocking: { conjunct: string } }

export interface LensEvent {
  /** Absent on the wire: the route already says which face or solid. */
  entity?: { face?: string; solid?: string }
  stage: 'import' | 'repair' | 'mesh' | 'certify'
  kind: LensKind
  step_instance: number | null
  why: string
  measured: [string, number][]
}

/** `GET /api/lens/{part}` */
export interface LensPart {
  solid_id?: number
  /** The certificate's verdict. Absent only in hand-built fixtures. */
  sound?: boolean
  overview: { kind: string; count: number }[]
  faces: { face_id: number; pid: string | null; status: LensStatus }[]
  solid_events: LensEvent[]
}

/** `GET /api/lens/{part}/face/{face_id}` */
export interface LensFace {
  face_id: number
  pid: string | null
  status: LensStatus
  events: LensEvent[]
}

/** Kernel face ids to tint: amber where the kernel repaired, red where a
 *  certificate check is blocked. As-written faces are left untinted. */
export function lensFaceTints(part: LensPart): { repaired: number[]; blocking: number[] } {
  const pick = (s: LensStatus) =>
    part.faces
      .filter((f) => f.status === s)
      .map((f) => f.face_id)
      .sort((a, b) => a - b)
  return { repaired: pick('repaired'), blocking: pick('blocking') }
}

/** The one-line verdict for the whole part. A solid-level blocker (one no
 *  face can be pointed at, e.g. the Euler count) still blocks it. */
export function lensSummary(part: LensPart): { state: LensStatus; text: string } {
  const blockingFaces = part.faces.filter((f) => f.status === 'blocking').length
  const solidBlocks = part.solid_events.some((e) => typeof e.kind === 'object' && 'blocking' in e.kind)
  if (blockingFaces > 0) {
    return { state: 'blocking', text: `${blockingFaces} face${blockingFaces === 1 ? '' : 's'} block${blockingFaces === 1 ? 's' : ''} the certificate` }
  }
  if (solidBlocks || part.sound === false) return { state: 'blocking', text: 'The part does not certify' }
  const repaired = part.faces.filter((f) => f.status === 'repaired').length
  if (repaired > 0) {
    return { state: 'repaired', text: `${repaired} face${repaired === 1 ? '' : 's'} repaired · sound` }
  }
  return { state: 'as_written', text: 'Imported as written · sound' }
}

const TITLES: Record<string, string> = {
  loop_reversed: 'Loop reversed',
  outer_loop_chosen: 'Outer loop chosen',
  profile_rescaled: 'Profile rescaled',
  edge_tolerance_raised: 'Edge tolerance raised',
  loop_reversed_to_match_neighbours: 'Loop reversed to match its neighbours',
  void_shell_reversed: 'Void shell turned inside out',
}

export interface LensStoryRow {
  mark: 'tick' | 'amber' | 'cross'
  title: string
  why: string
  numbers: string
  source: string | null
}

/** One face's story, blockers first (what to act on), then what the kernel
 *  repaired, each with the rule it followed and the numbers it measured. */
export function lensStoryRows(events: LensEvent[]): LensStoryRow[] {
  const rows = events.map((e): LensStoryRow & { order: number } => {
    const blocking = typeof e.kind === 'object' ? e.kind.blocking.conjunct : null
    return {
      order: blocking ? 0 : 1,
      mark: blocking ? 'cross' : 'amber',
      title: blocking ? `Blocks: ${blocking}` : (TITLES[e.kind as string] ?? String(e.kind)),
      why: e.why,
      numbers: e.measured.map(([k, v]) => `${k} ${v}`).join(' · '),
      source: e.step_instance === null ? null : `#${e.step_instance}`,
    }
  })
  rows.sort((a, b) => a.order - b.order)
  return rows.map(({ order: _order, ...r }) => r)
}

const API_HOST = (import.meta as { env?: Record<string, string | undefined> }).env?.VITE_API_URL || ''

async function getJson<T>(path: string): Promise<T> {
  const r = await fetch(`${API_HOST}${path}`)
  if (!r.ok) throw new Error(`${path} ${r.status}`)
  return (await r.json()) as T
}

/** The part's overview and every face's status. `part` is the kernel solid id
 *  or the scene object's UUID. */
export function fetchLensPart(part: number | string): Promise<LensPart> {
  return getJson<LensPart>(`/api/lens/${part}`)
}

/** One face's story. */
export function fetchLensFace(part: number | string, faceId: number): Promise<LensFace> {
  return getJson<LensFace>(`/api/lens/${part}/face/${faceId}`)
}
