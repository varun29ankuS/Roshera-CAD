/**
 * A part's properties, as the Properties panel shows them: the kernel's own
 * numbers (GET /api/agent/parts/{id}, /mass, /features, /perception), read
 * the way an engineer reads a spec sheet.
 */

const API_HOST: string =
  (import.meta as { env?: { VITE_API_URL?: string } }).env?.VITE_API_URL ?? ''

/** A number without float noise or trailing zeros, with a true minus. */
function num(v: number, places = 2): string {
  const r = Number(v.toFixed(places))
  return String(Object.is(r, -0) ? 0 : r).replace('-', '−')
}

/** Mass in kg -> "516 g" under a kilogram, "2.35 kg" above. */
export function formatMass(kg: number): string {
  if (kg < 1) {
    const g = kg * 1000
    return `${g >= 100 ? Math.round(g) : num(g, 1)} g`
  }
  return `${num(kg, 2)} kg`
}

/** Volume in mm³ -> cm³. */
export function formatVolume(mm3: number): string {
  return `${num(mm3 / 1000, 1)} cm³`
}

/** Area in mm² -> cm². */
export function formatArea(mm2: number): string {
  return `${num(mm2 / 100, 1)} cm²`
}

/** The bounding size, W × D × H in mm. */
export function formatSize(d: readonly number[]): string {
  return `${d.map((v) => num(v, 3)).join(' × ')} mm`
}

/** A point in mm, float noise dropped. */
export function formatPoint3(p: readonly number[]): string {
  return `(${p.map((v) => num(v, 2)).join(', ')})`
}

/** The part's cylinder diameters, biggest first: "Ø80 · Ø30 · 4× Ø8". */
export function diameterList(ds: { diameter: number; count: number }[]): string {
  if (ds.length === 0) return 'none'
  return [...ds]
    .sort((a, b) => b.diameter - a.diameter)
    .map((d) => `${d.count > 1 ? `${d.count}× ` : ''}Ø${num(d.diameter, 3)}`)
    .join(' · ')
}

export interface PartReport {
  name: string
  dimensions_world: number[]
  center_world: number[]
  world_bbox_min: number[]
  world_bbox_max: number[]
  volume: number
  surface_area: number
  topology: { face_count: number; edge_count: number; vertex_count: number }
  material: { name: string; density: number }
}

export interface MassReport {
  mass: number
  center_of_mass: number[]
  principal_moments: number[]
}

export interface PerceptionReport {
  sound: boolean
  status: string
  verified: boolean
  verdict: string
}

export interface FaceReport {
  id: number
  surface_type: string
  area: number
  neighbour_face_ids: number[]
}

export interface PartProperties {
  part: PartReport
  mass: MassReport | null
  perception: PerceptionReport | null
  diameters: { diameter: number; count: number }[]
}

async function json<T>(path: string): Promise<T> {
  const r = await fetch(`${API_HOST}${path}`)
  if (!r.ok) throw new Error(`HTTP ${r.status}`)
  return (await r.json()) as T
}

/** Everything the panel shows about one part. The part report is required;
 *  mass, verdict and diameters each fall back to "not read" on their own. */
export async function fetchPartProperties(solidId: number): Promise<PartProperties> {
  const [part, mass, perception, features] = await Promise.all([
    json<{ location: { dimensions_world: number[]; center_world: number[] } } & Omit<PartReport, 'dimensions_world' | 'center_world'>>(
      `/api/agent/parts/${solidId}`,
    ),
    json<MassReport>(`/api/agent/parts/${solidId}/mass`).catch(() => null),
    json<PerceptionReport>(`/api/agent/parts/${solidId}/perception`).catch(() => null),
    json<{ diameters?: { diameter: number; count: number }[] }>(`/api/agent/parts/${solidId}/features`).catch(() => null),
  ])
  return {
    part: { ...part, dimensions_world: part.location.dimensions_world, center_world: part.location.center_world },
    mass,
    perception,
    diameters: features?.diameters ?? [],
  }
}

export function fetchFace(faceId: number): Promise<FaceReport> {
  return json<FaceReport>(`/api/agent/faces/${faceId}`)
}
