/**
 * Editing a feature's sizes. The server lists every size an edit can change
 * (`GET /api/timeline/parameters/{branch}`, the kernel's own rule) with its
 * value now; a change goes to `POST /api/timeline/mould`, which tries it on
 * a scratch copy and refuses, with the kernel's reason, when a later feature
 * would break or the part would not be sound.
 */

const API_HOST: string =
  (import.meta as { env?: { VITE_API_URL?: string } }).env?.VITE_API_URL ?? ''

export interface EditableParameter {
  sequence: number
  event_id: string
  command: string
  key: string
  value: number
  edited: boolean
}

/** The parts of a chapter this needs: its steps, and its instances when it
 *  is a pattern. */
export interface FeatureShape {
  events: number[]
  instances: { events: number[] }[]
}

export interface FeatureField {
  key: string
  label: string
  unit: string
  value: number
  edited: boolean
  /** Every recorded parameter this field sets (one per instance of a pattern). */
  targets: { event_id: string; key: string }[]
}

const LABELS: Record<string, string> = {
  radius: 'Radius',
  height: 'Height',
  width: 'Width',
  depth: 'Depth',
  length: 'Length',
  distance: 'Distance',
  distance1: 'Distance',
  distance2: 'Second distance',
  thickness: 'Thickness',
  angle: 'Angle',
  major_radius: 'Major radius',
  minor_radius: 'Minor radius',
  base_x: 'Position X',
  base_y: 'Position Y',
  base_z: 'Position Z',
  center_x: 'Centre X',
  center_y: 'Centre Y',
  center_z: 'Centre Z',
  x: 'X',
  y: 'Y',
  z: 'Z',
}

function label(key: string): string {
  if (LABELS[key]) return LABELS[key]
  const t = key.replace(/_/g, ' ')
  return t.charAt(0).toUpperCase() + t.slice(1)
}

function unit(key: string): string {
  if (/angle|_deg$/.test(key)) return '°'
  if (/count|instances|segments|number/.test(key)) return ''
  return 'mm'
}

/**
 * The fields to show for a feature: each size of the steps it is made of.
 * For a pattern, a size is one field only when every instance has it with
 * the same value (a hole's radius), set on all instances together; sizes
 * that differ per instance (each hole's position) are not one field.
 */
export function featureFields(feature: FeatureShape, params: EditableParameter[]): FeatureField[] {
  const groups =
    feature.instances.length > 0 ? feature.instances.map((i) => new Set(i.events)) : [new Set(feature.events)]
  const perGroup = groups.map((g) => params.filter((p) => g.has(p.sequence)))
  const first = perGroup[0] ?? []
  const fields: FeatureField[] = []
  for (const p of first) {
    if (fields.some((f) => f.key === p.key)) continue
    const matches = perGroup.map((list) => list.find((q) => q.key === p.key && q.command === p.command))
    if (matches.some((m) => !m || m.value !== p.value)) continue
    const all = matches as EditableParameter[]
    fields.push({
      key: p.key,
      label: label(p.key),
      unit: unit(p.key),
      value: p.value,
      edited: all.some((m) => m.edited),
      targets: all.map((m) => ({ event_id: m.event_id, key: m.key })),
    })
  }
  return fields
}

/** The mould request setting `field` to `value` (linked across a pattern). */
export function mouldBody(field: FeatureField, value: number) {
  const [head, ...rest] = field.targets
  return {
    target_event_id: head.event_id,
    parameter: head.key,
    value,
    linked: rest.map((t) => ({ target_event_id: t.event_id, parameter: t.key })),
  }
}

/** A typed size, or null when it is not a finite number. */
export function parseSize(text: string): number | null {
  const m = /^\s*(-?\d+(?:\.\d+)?|-?\.\d+)\s*(mm|°)?\s*$/i.exec(text)
  if (!m) return null
  const v = Number(m[1])
  return Number.isFinite(v) ? v : null
}

export async function fetchEditableParameters(branch: string): Promise<EditableParameter[]> {
  const r = await fetch(`${API_HOST}/api/timeline/parameters/${encodeURIComponent(branch)}`)
  if (!r.ok) throw new Error(`HTTP ${r.status}`)
  const body = (await r.json()) as { parameters?: EditableParameter[] }
  return body.parameters ?? []
}

/** Apply an edit. A refusal comes back as the kernel's reason. */
export async function applyEdit(body: ReturnType<typeof mouldBody>): Promise<{ ok: true } | { ok: false; reason: string }> {
  try {
    const r = await fetch(`${API_HOST}/api/timeline/mould`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    })
    if (r.ok) return { ok: true }
    const err = (await r.json().catch(() => ({}))) as { reason?: string; error?: string }
    return { ok: false, reason: err.reason ?? err.error ?? `HTTP ${r.status}` }
  } catch (e) {
    return { ok: false, reason: e instanceof Error ? e.message : String(e) }
  }
}
