import type { EntityKind, EntityRef, UserContextReport, Vec3 } from './user-context.ts'

/**
 * THE SELECTION TRAVELS WITH THE MESSAGE (Varun, 2026-11-18)
 * ===========================================================
 * When the person sends the agent a message, what they have selected in the
 * viewport goes with it: shown as chips on the composer (each removable for
 * this ask), frozen on the sent line, and written into the text the agent
 * receives as one fenced block. "Fillet this edge" then names its edge by
 * kernel id, and stays resolvable after the live context
 * (`GET /api/agent/user-context`) has moved on to the next thing the person
 * clicks.
 *
 * # Where the chips come from
 * The last report the server acknowledged (`user-context-store`'s
 * `acknowledged`): what an agent reading the live context would be told,
 * the same source as the status bar's "Agent sees". Sharing paused means
 * nothing is attached, decided from the local flag so the chips vanish the
 * moment the person pauses, before the server has heard. Only the
 * selection travels, never the hover: at the moment of sending the cursor
 * is on the composer, not on the part.
 *
 * # What the agent receives
 * The words, a blank line, then:
 *
 *     ```roshera:selection
 *     {"kind":"face","part":"Flange","object_id":"…","solid_id":7,"id":56,"pid":"…","part_pid":"…","surface":"plane","area_mm2":1256.637}
 *     {"more":3}
 *     ```
 *
 * One JSON object per selected entity, keys always in the order of
 * `KEY_ORDER`, entities in selection order, numbers rounded to 3 decimals:
 * the same selection is always the same bytes. `solid_id` is the kernel
 * solid id agent tools take; `id` the kernel face / edge / vertex id; `pid`
 * and `part_pid` the kernel's persistent ids. A field is present only when
 * the kernel gave it (an edge the kernel has not answered for has no `id`;
 * a vertex has no persistent id; a part whose size was not read has no
 * `size_mm`) -- never approximated from the tessellation. A last
 * `{"more":n}` line counts selected entities not listed.
 *
 * # Read back
 * The board keeps only the line's text, so the sent line's chips are
 * derived from it (`splitSelection`), after a reload too. Only a trailing,
 * well-formed block is read; anything else is the person's text, shown as
 * written. The fence is deliberately NOT a card kind (`blackboard-cards.ts`
 * `CARD_KINDS`): it carries no verdict and must never colour a line or a
 * turn as evidence.
 */

export const SELECTION_FENCE = 'roshera:selection'

/** The most entities one message carries; the rest are counted. */
export const MAX_ATTACHED = 12

/** The key the "+n more" chip is removed by. */
export const MORE_KEY = 'more'

/** One selected entity as the agent is told it and the chip shows it. */
export interface SelectionItem {
  kind: EntityKind
  /** The part's name as the person sees it. */
  part: string
  /** The app's id for the part (what the live context reports). */
  object_id: string
  /** The kernel solid id: what agent tools take as the part id. */
  solid_id?: number
  /** The kernel id of the face, edge or vertex. */
  id?: number
  /** The kernel's persistent id of the face or edge. */
  pid?: string
  /** The kernel's persistent id of the part. */
  part_pid?: string
  /** Face: the kernel's surface type, and its area. */
  surface?: string
  area_mm2?: number
  /** Edge: the kernel's curve kind, and its length. */
  curve?: string
  length_mm?: number
  /** Part: its size (W × D × H) as the kernel reports it. */
  size_mm?: Vec3
  /** Vertex: where it is. */
  at_mm?: Vec3
}

/** What is attached to one message. */
export interface SelectionAttachment {
  items: SelectionItem[]
  /** Selected entities not listed (over `MAX_ATTACHED`). */
  more: number
}

/** A chip on the composer: the item, and the key that removes it. */
export interface ComposerChip {
  key: string
  item: SelectionItem
}

/** What the kernel said about one entity (GET /api/agent/user-context, and
 *  the part report for a part's size). Every field is optional: a field
 *  the kernel did not give is absent. */
export interface Resolution {
  solid_id?: number
  part_name?: string
  part_pid?: string
  pid?: string
  surface?: string
  area_mm2?: number
  curve?: string
  length_mm?: number
  at_mm?: Vec3
  size_mm?: Vec3
}

const KINDS: readonly EntityKind[] = ['part', 'face', 'edge', 'vertex']

/** The serialized key order. */
const KEY_ORDER = [
  'kind',
  'part',
  'object_id',
  'solid_id',
  'id',
  'pid',
  'part_pid',
  'surface',
  'area_mm2',
  'curve',
  'length_mm',
  'size_mm',
  'at_mm',
] as const satisfies readonly (keyof SelectionItem)[]

/** How an entity is told apart from every other (and removed by). */
export function entityKey(e: Pick<EntityRef, 'object_id' | 'kind' | 'id'>): string {
  return `${e.object_id}|${e.kind}|${e.id ?? ''}`
}

function round3(v: number): number {
  const r = Math.round(v * 1000) / 1000
  return Object.is(r, -0) ? 0 : r
}

function isFiniteNumber(v: unknown): v is number {
  return typeof v === 'number' && Number.isFinite(v)
}

function vec3(v: unknown): Vec3 | undefined {
  if (!Array.isArray(v) || v.length !== 3 || !v.every(isFiniteNumber)) return undefined
  return [round3(v[0]), round3(v[1]), round3(v[2])]
}

function str(v: unknown): string | undefined {
  return typeof v === 'string' && v !== '' ? v : undefined
}

function int(v: unknown): number | undefined {
  return Number.isInteger(v) ? (v as number) : undefined
}

/**
 * What the kernel resolved, keyed by `entityKey`, from the body of
 * `GET /api/agent/user-context`. The part's own facts (solid id, name,
 * persistent id) are taken whenever present; the entity's (its persistent
 * id and measure) only when the kernel resolved it -- a face a later
 * operation replaced has no numbers to give.
 */
export function resolutionsFromContext(body: unknown): Map<string, Resolution> {
  const out = new Map<string, Resolution>()
  if (typeof body !== 'object' || body === null) return out
  const selection = (body as { selection?: unknown }).selection
  if (!Array.isArray(selection)) return out
  for (const raw of selection) {
    if (typeof raw !== 'object' || raw === null) continue
    const e = raw as Record<string, unknown>
    const objectId = str(e.object_id)
    const kind = KINDS.find((k) => k === e.kind)
    if (objectId === undefined || kind === undefined) continue
    const r: Resolution = {}
    const solid = int(e.solid_id)
    if (solid !== undefined) r.solid_id = solid
    const partName = str(e.part_name)
    if (partName !== undefined) r.part_name = partName
    const partPid = str(e.part_persistent_id)
    if (partPid !== undefined) r.part_pid = partPid
    if (e.resolved === true) {
      const pid = str(e.persistent_id)
      if (pid !== undefined) r.pid = pid
      if (kind === 'face') {
        const surface = str(e.surface_type)
        if (surface !== undefined) r.surface = surface
        if (isFiniteNumber(e.area)) r.area_mm2 = round3(e.area)
      } else if (kind === 'edge') {
        const curve = str(e.curve_kind)
        if (curve !== undefined) r.curve = curve
        if (isFiniteNumber(e.length)) r.length_mm = round3(e.length)
      } else if (kind === 'vertex') {
        const at = vec3(e.position)
        if (at !== undefined) r.at_mm = at
      }
    }
    out.set(entityKey({ object_id: objectId, kind, id: int(e.id) }), r)
  }
  return out
}

/** The part's size from its kernel report's `dimensions_world`. */
export function sizeFromPartReport(body: unknown): Vec3 | undefined {
  if (typeof body !== 'object' || body === null) return undefined
  const location = (body as { location?: unknown }).location
  if (typeof location !== 'object' || location === null) return undefined
  return vec3((location as { dimensions_world?: unknown }).dimensions_world)
}

export interface ChipInput {
  /** The person's sharing choice now (the store's flag). */
  sharing: boolean
  /** The last report the server acknowledged. */
  report: UserContextReport | null
  /** The part's name in the scene. */
  nameOf: (objectId: string) => string | null
  /** The part's kernel solid id in the scene. */
  solidIdOf: (objectId: string) => number | undefined
  /** What the kernel resolved, keyed by `entityKey`. */
  resolved: ReadonlyMap<string, Resolution>
}

function itemOf(e: EntityRef, input: ChipInput): SelectionItem {
  const r = input.resolved.get(entityKey(e)) ?? {}
  const partRes = e.kind === 'part' ? r : input.resolved.get(entityKey({ object_id: e.object_id, kind: 'part' }))
  const item: SelectionItem = {
    kind: e.kind,
    part: input.nameOf(e.object_id) ?? r.part_name ?? 'part',
    object_id: e.object_id,
  }
  const solid = input.solidIdOf(e.object_id) ?? r.solid_id
  if (solid !== undefined) item.solid_id = solid
  if (e.kind !== 'part' && e.id !== undefined) item.id = e.id
  if (r.pid !== undefined && e.kind !== 'part') item.pid = r.pid
  const partPid = r.part_pid ?? partRes?.part_pid
  if (partPid !== undefined) item.part_pid = partPid
  if (e.kind === 'face') {
    if (r.surface !== undefined) item.surface = r.surface
    if (r.area_mm2 !== undefined) item.area_mm2 = r.area_mm2
  } else if (e.kind === 'edge') {
    if (r.curve !== undefined) item.curve = r.curve
    if (r.length_mm !== undefined) item.length_mm = r.length_mm
  } else if (e.kind === 'vertex') {
    if (r.at_mm !== undefined) item.at_mm = r.at_mm
  } else if (r.size_mm !== undefined) {
    item.size_mm = r.size_mm
  }
  return item
}

/** The chips the composer offers now: the first `MAX_ATTACHED` selected
 *  entities, and how many more are selected. Paused: none. */
export function composerChips(input: ChipInput): { chips: ComposerChip[]; more: number } {
  const report = input.report
  if (!input.sharing || report === null || !report.sharing) return { chips: [], more: 0 }
  const selection = report.selection ?? []
  const total = Math.max(report.selection_total ?? selection.length, selection.length)
  const shown = selection.slice(0, MAX_ATTACHED)
  return {
    chips: shown.map((e) => ({ key: entityKey(e), item: itemOf(e, input) })),
    more: total - shown.length,
  }
}

/** What one message carries: the chips not removed, and the count of the
 *  rest unless its chip was removed (`MORE_KEY`). */
export function attachmentOf(
  chips: readonly ComposerChip[],
  more: number,
  removed: ReadonlySet<string>,
): SelectionAttachment {
  return {
    items: chips.filter((c) => !removed.has(c.key)).map((c) => c.item),
    more: removed.has(MORE_KEY) ? 0 : more,
  }
}

/** JSON with every backtick escaped, so no value can close the fence. */
function line(obj: Record<string, unknown>): string {
  return JSON.stringify(obj).replace(/`/g, '\\u0060')
}

/** The fenced block, or '' when nothing is attached. */
export function serializeSelection(att: SelectionAttachment | null): string {
  if (att === null || (att.items.length === 0 && att.more <= 0)) return ''
  const rows = att.items.map((item) => {
    const ordered: Record<string, unknown> = {}
    for (const k of KEY_ORDER) if (item[k] !== undefined) ordered[k] = item[k]
    return line(ordered)
  })
  if (att.more > 0) rows.push(line({ more: att.more }))
  return ['```' + SELECTION_FENCE, ...rows, '```'].join('\n')
}

/** The message the agent receives: the words, then the block. Nothing
 *  attached: the words, byte for byte. */
export function attachSelection(text: string, att: SelectionAttachment | null): string {
  const block = serializeSelection(att)
  return block === '' ? text : `${text}\n\n${block}`
}

const TRAILING_BLOCK = new RegExp('\\n*```' + SELECTION_FENCE + '\\n([\\s\\S]*?)\\n```\\s*$')

function parseItem(raw: unknown): SelectionItem | null {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) return null
  const o = raw as Record<string, unknown>
  const kind = KINDS.find((k) => k === o.kind)
  const part = typeof o.part === 'string' ? o.part : undefined
  const objectId = str(o.object_id)
  if (kind === undefined || part === undefined || objectId === undefined) return null
  const item: SelectionItem = { kind, part, object_id: objectId }
  for (const k of Object.keys(o)) {
    if (!(KEY_ORDER as readonly string[]).includes(k)) return null
  }
  const ints = ['solid_id', 'id'] as const
  for (const k of ints) {
    if (o[k] === undefined) continue
    const v = int(o[k])
    if (v === undefined) return null
    item[k] = v
  }
  const strs = ['pid', 'part_pid', 'surface', 'curve'] as const
  for (const k of strs) {
    if (o[k] === undefined) continue
    const v = str(o[k])
    if (v === undefined) return null
    item[k] = v
  }
  const nums = ['area_mm2', 'length_mm'] as const
  for (const k of nums) {
    if (o[k] === undefined) continue
    if (!isFiniteNumber(o[k])) return null
    item[k] = o[k] as number
  }
  const vecs = ['size_mm', 'at_mm'] as const
  for (const k of vecs) {
    if (o[k] === undefined) continue
    const v = vec3(o[k])
    if (v === undefined) return null
    item[k] = v
  }
  return item
}

/**
 * A stored message read back: its words, and what was attached when a
 * trailing, well-formed `roshera:selection` block ends it. Anything else --
 * no block, a block mid-text, a line that does not parse -- is the text as
 * written, with no attachment: never a guessed chip.
 */
export function splitSelection(text: string): { prose: string; attachment: SelectionAttachment | null } {
  const m = TRAILING_BLOCK.exec(text)
  if (!m) return { prose: text, attachment: null }
  const rows = m[1].split('\n')
  const items: SelectionItem[] = []
  let more = 0
  for (let i = 0; i < rows.length; i++) {
    let raw: unknown
    try {
      raw = JSON.parse(rows[i])
    } catch {
      return { prose: text, attachment: null }
    }
    if (typeof raw === 'object' && raw !== null && 'more' in raw) {
      const n = (raw as { more: unknown }).more
      const keys = Object.keys(raw)
      if (i !== rows.length - 1 || keys.length !== 1 || !Number.isInteger(n) || (n as number) <= 0) {
        return { prose: text, attachment: null }
      }
      more = n as number
      continue
    }
    const item = parseItem(raw)
    if (item === null) return { prose: text, attachment: null }
    items.push(item)
  }
  if (items.length === 0 && more === 0) return { prose: text, attachment: null }
  return { prose: text.slice(0, m.index), attachment: { items, more } }
}

/** A message's words alone, for places that show it as one short text
 *  (the queue strip, the collapsed board). */
export function messageText(text: string): string {
  return splitSelection(text).prose
}

function num(v: number): string {
  const r = Math.round(v * 100) / 100
  return String(Object.is(r, -0) ? 0 : r).replace('-', '−')
}

/** What a chip says: the entity ("Flange · face 56"), and the kernel's
 *  measure of it ("plane · 1256.64 mm²"), '' when the kernel gave none. */
export function chipText(item: SelectionItem): { title: string; measure: string } {
  let title: string
  if (item.kind === 'part') title = item.part
  else if (item.id === undefined) title = `${item.part} · ${item.kind === 'edge' ? 'an' : 'a'} ${item.kind}`
  else title = `${item.part} · ${item.kind} ${item.id}`

  const parts: string[] = []
  if (item.kind === 'face') {
    if (item.surface) parts.push(item.surface)
    if (item.area_mm2 !== undefined) parts.push(`${num(item.area_mm2)} mm²`)
  } else if (item.kind === 'edge') {
    if (item.curve) parts.push(item.curve)
    if (item.length_mm !== undefined) parts.push(`${num(item.length_mm)} mm`)
  } else if (item.kind === 'vertex') {
    if (item.at_mm) parts.push(`(${item.at_mm.map(num).join(', ')}) mm`)
  } else if (item.size_mm) {
    parts.push(`${item.size_mm.map(num).join(' × ')} mm`)
  }
  return { title, measure: parts.join(' · ') }
}

/** The ids a chip's tooltip spells out, so the person can see what the
 *  agent will be told. */
export function chipIds(item: SelectionItem): string {
  const ids: string[] = []
  if (item.solid_id !== undefined) ids.push(`solid ${item.solid_id}`)
  if (item.kind !== 'part') ids.push(item.id !== undefined ? `${item.kind} ${item.id}` : `${item.kind} id not known yet`)
  if (item.pid) ids.push(`pid ${item.pid}`)
  else if (item.kind === 'part' && item.part_pid) ids.push(`pid ${item.part_pid}`)
  return ids.join(' · ')
}
